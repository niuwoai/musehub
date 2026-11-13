//! Release draft + publish bound to pipeline run and merge commit.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::artifacts;
use crate::error::AppError;
use crate::security::{validate_git_oid, validate_oci_image_ref};

#[derive(Serialize)]
pub struct ReleaseView {
    pub id: Uuid,
    pub version: String,
    pub tag_name: String,
    pub commit_oid: String,
    pub pipeline_run_id: Uuid,
    pub state: String,
    pub manifest_sha256: Option<String>,
    pub oci_image_ref: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct PublishReleaseRequest {
    /// Digest-pinned OCI image for compose-runner deploys (required when using runner in prod paths).
    pub oci_image_ref: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateReleaseRequest {
    pub version: String,
    pub tag_name: String,
    pub commit_oid: String,
    pub pipeline_run_id: Uuid,
    pub pr_id: Option<Uuid>,
}

pub async fn create_release_core(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    body: CreateReleaseRequest,
) -> Result<ReleaseView, AppError> {
    validate_git_oid(&body.commit_oid).map_err(AppError::Validation)?;
    let pipe = sqlx::query_as::<_, (String, String)>(
        "SELECT state, source_oid FROM pipeline_runs WHERE id = $1 AND repo_id = $2",
    )
    .bind(body.pipeline_run_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pipe_state, source_oid)) = pipe else {
        return Err(AppError::NotFound);
    };
    if pipe_state != "succeeded" {
        return Err(AppError::Validation(
            "release requires succeeded pipeline run".to_string(),
        ));
    }
    if source_oid != body.commit_oid {
        return Err(AppError::Validation(
            "pipeline source_oid must match release commit".to_string(),
        ));
    }
    let artifact_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM artifacts WHERE pipeline_run_id = $1 AND state = 'ready'
         AND length(sha256) = 64 AND byte_length > 0",
    )
    .bind(body.pipeline_run_id)
    .fetch_one(&state.pool)
    .await?;
    if artifact_count.0 == 0 {
        return Err(AppError::Validation(
            "release requires verified ready artifacts".to_string(),
        ));
    }
    artifacts::verify_ready_artifacts_for_pipeline(&state.pool, body.pipeline_run_id).await?;
    let pipeline_oci = if state.require_pipeline_oci {
        Some(artifacts::require_pipeline_oci_artifact(&state.pool, body.pipeline_run_id).await?)
    } else {
        artifacts::pipeline_bound_oci_image_ref(&state.pool, body.pipeline_run_id).await?
    };
    let release_kind: (String,) =
        sqlx::query_as("SELECT run_kind FROM pipeline_runs WHERE id = $1")
            .bind(body.pipeline_run_id)
            .fetch_one(&state.pool)
            .await?;
    if release_kind.0 != "release_build" {
        return Err(AppError::Validation(
            "release requires a release_build pipeline run".to_string(),
        ));
    }
    if matches!(
        pipe_state.as_str(),
        "failed" | "canceled" | "cancel_requested"
    ) {
        return Err(AppError::Validation(
            "release blocked: pipeline did not succeed".to_string(),
        ));
    }
    let release_id = Uuid::new_v4();
    let mut manifest: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    manifest.insert("commit_oid".to_string(), body.commit_oid.clone().into());
    manifest.insert(
        "pipeline_run_id".to_string(),
        body.pipeline_run_id.to_string().into(),
    );
    if let Some(ref oci) = pipeline_oci {
        manifest.insert(
            "pipeline_oci_image_ref".to_string(),
            serde_json::Value::String(oci.clone()),
        );
    }
    let manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|e| AppError::Other(e.to_string()))?;
    let manifest_sha256 = Sha256::digest(&manifest_bytes);
    let manifest_hex: String = manifest_sha256
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id,
         manifest_sha256, state)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'draft')",
    )
    .bind(release_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&body.version)
    .bind(&body.tag_name)
    .bind(&body.commit_oid)
    .bind(body.pipeline_run_id)
    .bind(&manifest_hex)
    .execute(&mut *tx)
    .await?;
    if let Some(pr_id) = body.pr_id {
        sqlx::query(
            "INSERT INTO release_pr_links (id, org_id, repo_id, release_id, pr_id, merged_commit_oid)
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(Uuid::new_v4())
        .bind(org_id)
        .bind(repo_id)
        .bind(release_id)
        .bind(pr_id)
        .bind(&body.commit_oid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ReleaseView {
        id: release_id,
        version: body.version,
        tag_name: body.tag_name,
        commit_oid: body.commit_oid,
        pipeline_run_id: body.pipeline_run_id,
        state: "draft".to_string(),
        manifest_sha256: Some(manifest_hex),
        oci_image_ref: None,
    })
}

pub async fn create_release(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Json(body): Json<CreateReleaseRequest>,
) -> Result<(StatusCode, Json<ReleaseView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let view = create_release_core(&state, org_id, repo_id, body).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

pub async fn publish_release_core(
    state: &Arc<crate::AppState>,
    repo_id: Uuid,
    release_id: Uuid,
    body: PublishReleaseRequest,
) -> Result<ReleaseView, AppError> {
    let draft = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
        "SELECT pipeline_run_id, commit_oid, manifest_sha256 FROM releases
         WHERE id = $1 AND repo_id = $2 AND state = 'draft'",
    )
    .bind(release_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pipeline_run_id, commit_oid, _old_manifest)) = draft else {
        return Err(AppError::Validation(
            "release not in draft or not found".to_string(),
        ));
    };
    let bound = artifacts::pipeline_bound_oci_image_ref(&state.pool, pipeline_run_id).await?;
    let oci_to_set: Option<String> = if state.require_pipeline_oci {
        let expected = bound.ok_or_else(|| {
            AppError::Validation("publish requires pipeline-built OCI artifact".to_string())
        })?;
        if let Some(ref submitted) = body.oci_image_ref {
            validate_oci_image_ref(submitted).map_err(AppError::Validation)?;
            if submitted != &expected {
                return Err(AppError::Validation(
                    "oci_image_ref does not match pipeline artifact (tamper)".to_string(),
                ));
            }
        }
        Some(expected)
    } else if let Some(ref submitted) = body.oci_image_ref {
        validate_oci_image_ref(submitted).map_err(AppError::Validation)?;
        if let Some(ref pipeline_ref) = bound {
            if submitted != pipeline_ref {
                return Err(AppError::Validation(
                    "oci_image_ref must match pipeline artifact when present".to_string(),
                ));
            }
        }
        Some(submitted.clone())
    } else {
        bound
    };
    let mut manifest: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    manifest.insert("commit_oid".into(), commit_oid.into());
    manifest.insert("pipeline_run_id".into(), pipeline_run_id.to_string().into());
    if let Some(ref oci) = oci_to_set {
        manifest.insert(
            "pipeline_oci_image_ref".into(),
            serde_json::Value::String(oci.clone()),
        );
    }
    let manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|e| AppError::Other(e.to_string()))?;
    let manifest_hex: String = Sha256::digest(&manifest_bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    let updated = sqlx::query(
        "UPDATE releases SET state = 'published',
         oci_image_ref = $3,
         manifest_sha256 = $4,
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND state = 'draft'",
    )
    .bind(release_id)
    .bind(repo_id)
    .bind(oci_to_set)
    .bind(manifest_hex)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Validation(
            "release not in draft or not found".to_string(),
        ));
    }
    let row = sqlx::query_as::<_, (Uuid, String, String, String, Uuid, String, Option<String>, Option<String>)>(
        "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256, oci_image_ref
         FROM releases WHERE id = $1",
    )
    .bind(release_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(ReleaseView {
        id: row.0,
        version: row.1,
        tag_name: row.2,
        commit_oid: row.3,
        pipeline_run_id: row.4,
        state: row.5,
        manifest_sha256: row.6,
        oci_image_ref: row.7,
    })
}

pub async fn list_releases(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
) -> Result<Json<Vec<ReleaseView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, Uuid, String, Option<String>, Option<String>)>(
        "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256, oci_image_ref
         FROM releases WHERE repo_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(repo_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| ReleaseView {
                id: row.0,
                version: row.1,
                tag_name: row.2,
                commit_oid: row.3,
                pipeline_run_id: row.4,
                state: row.5,
                manifest_sha256: row.6,
                oci_image_ref: row.7,
            })
            .collect(),
    ))
}

pub async fn publish_release(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, release_id)): AxumPath<(String, String, Uuid)>,
    body: Option<Json<PublishReleaseRequest>>,
) -> Result<Json<ReleaseView>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let view = publish_release_core(&state, repo_id, release_id, body).await?;
    Ok(Json(view))
}
