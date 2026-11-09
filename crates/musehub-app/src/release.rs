//! Release draft + publish bound to pipeline run and merge commit.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;
use crate::security::validate_git_oid;

#[derive(Serialize)]
pub struct ReleaseView {
    pub id: Uuid,
    pub version: String,
    pub tag_name: String,
    pub commit_oid: String,
    pub pipeline_run_id: Uuid,
    pub state: String,
    pub manifest_sha256: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateReleaseRequest {
    pub version: String,
    pub tag_name: String,
    pub commit_oid: String,
    pub pipeline_run_id: Uuid,
    pub pr_id: Option<Uuid>,
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
    Ok((
        StatusCode::CREATED,
        Json(ReleaseView {
            id: release_id,
            version: body.version,
            tag_name: body.tag_name,
            commit_oid: body.commit_oid,
            pipeline_run_id: body.pipeline_run_id,
            state: "draft".to_string(),
            manifest_sha256: Some(manifest_hex),
        }),
    ))
}

pub async fn publish_release(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, release_id)): AxumPath<(String, String, Uuid)>,
) -> Result<Json<ReleaseView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let updated = sqlx::query(
        "UPDATE releases SET state = 'published', updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND state = 'draft'",
    )
    .bind(release_id)
    .bind(repo_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Validation(
            "release not in draft or not found".to_string(),
        ));
    }
    let row = sqlx::query_as::<_, (Uuid, String, String, String, Uuid, String, Option<String>)>(
        "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256
         FROM releases WHERE id = $1",
    )
    .bind(release_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(ReleaseView {
        id: row.0,
        version: row.1,
        tag_name: row.2,
        commit_oid: row.3,
        pipeline_run_id: row.4,
        state: row.5,
        manifest_sha256: row.6,
    }))
}
