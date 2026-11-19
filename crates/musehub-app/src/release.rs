//! Release draft + publish bound to pipeline run and merge commit.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::process::Command;
use uuid::Uuid;

use crate::artifacts;
use crate::error::AppError;
use crate::security::{validate_git_oid, validate_oci_image_ref};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseArtifactView {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub sha256: String,
    pub byte_length: i64,
}

#[derive(Serialize)]
pub struct ReleaseView {
    pub id: Uuid,
    pub version: String,
    pub tag_name: String,
    pub commit_oid: String,
    pub pipeline_run_id: Uuid,
    pub state: String,
    pub release_notes: String,
    pub assets: Vec<ReleaseArtifactView>,
    pub manifest_sha256: Option<String>,
    pub oci_image_ref: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct PublishReleaseRequest {
    /// Digest-pinned OCI image for compose-runner deploys (required when using runner in prod paths).
    pub oci_image_ref: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct CreateReleaseRequest {
    pub version: String,
    pub tag_name: String,
    pub commit_oid: String,
    pub pipeline_run_id: Uuid,
    pub pr_id: Option<Uuid>,
    #[serde(default)]
    pub release_notes: String,
    #[serde(default)]
    pub artifact_ids: Vec<Uuid>,
}

async fn validate_release_tag_name(tag_name: &str) -> Result<(), AppError> {
    let ref_name = format!("refs/tags/{tag_name}");
    let output = tokio::task::spawn_blocking(move || {
        Command::new("git")
            .args(["check-ref-format", &ref_name])
            .output()
    })
    .await
    .map_err(|error| AppError::Other(format!("validate release tag name: {error}")))?
    .map_err(|error| AppError::Other(format!("run git check-ref-format: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(AppError::Validation("invalid Git tag name".to_string()))
    }
}

async fn load_release_assets(
    pool: &sqlx::PgPool,
    release_id: Uuid,
) -> Result<Vec<ReleaseArtifactView>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, i64)>(
        "SELECT a.id, a.name, a.kind, a.sha256, a.byte_length
         FROM release_artifacts ra JOIN artifacts a ON a.id = ra.artifact_id
         WHERE ra.release_id = $1 AND a.state = 'ready' ORDER BY a.name",
    )
    .bind(release_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, kind, sha256, byte_length)| ReleaseArtifactView {
                id,
                name,
                kind,
                sha256,
                byte_length,
            },
        )
        .collect())
}

async fn find_release_by_version(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    version: &str,
) -> Result<Option<(ReleaseView, Option<String>)>, AppError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            Uuid,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        ),
    >(
        "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state,
         manifest_sha256, oci_image_ref, create_request_sha256, release_notes
         FROM releases WHERE repo_id = $1 AND version = $2",
    )
    .bind(repo_id)
    .bind(version)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let assets = load_release_assets(pool, row.0).await?;
    Ok(Some((
        ReleaseView {
            id: row.0,
            version: row.1,
            tag_name: row.2,
            commit_oid: row.3,
            pipeline_run_id: row.4,
            state: row.5,
            manifest_sha256: row.6,
            oci_image_ref: row.7,
            release_notes: row.9,
            assets,
        },
        row.8,
    )))
}

fn create_request_sha256(body: &CreateReleaseRequest) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(body).map_err(|error| AppError::Other(error.to_string()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub async fn create_release_core(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    body: CreateReleaseRequest,
) -> Result<ReleaseView, AppError> {
    validate_git_oid(&body.commit_oid).map_err(AppError::Validation)?;
    validate_release_tag_name(&body.tag_name).await?;
    if body.release_notes.len() > 64 * 1024 {
        return Err(AppError::Validation(
            "release notes exceed 64 KiB".to_string(),
        ));
    }
    let request_sha256 = create_request_sha256(&body)?;
    if let Some((existing, existing_hash)) =
        find_release_by_version(&state.pool, repo_id, &body.version).await?
    {
        return if existing_hash.as_deref() == Some(request_sha256.as_str()) {
            Ok(existing)
        } else {
            Err(AppError::Conflict(
                "release version already exists with different request parameters".to_string(),
            ))
        };
    }

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
    let ready_assets = sqlx::query_as::<_, (Uuid, String, String, String, i64)>(
        "SELECT id, name, kind, sha256, byte_length FROM artifacts
         WHERE pipeline_run_id = $1 AND state = 'ready'
         AND length(sha256) = 64 AND byte_length > 0 ORDER BY name",
    )
    .bind(body.pipeline_run_id)
    .fetch_all(&state.pool)
    .await?;
    let requested_ids = if body.artifact_ids.is_empty() {
        ready_assets.iter().map(|asset| asset.0).collect::<Vec<_>>()
    } else {
        body.artifact_ids.clone()
    };
    let unique_ids = requested_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let selected_assets = ready_assets
        .into_iter()
        .filter(|asset| unique_ids.contains(&asset.0))
        .collect::<Vec<_>>();
    if unique_ids.len() != requested_ids.len() || selected_assets.len() != unique_ids.len() {
        return Err(AppError::Validation(
            "release assets must be verified ready artifacts from the selected pipeline"
                .to_string(),
        ));
    }
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
    let mut manifest: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    manifest.insert("commit_oid".to_string(), body.commit_oid.clone().into());
    manifest.insert(
        "pipeline_run_id".to_string(),
        body.pipeline_run_id.to_string().into(),
    );
    manifest.insert(
        "release_notes".to_string(),
        body.release_notes.clone().into(),
    );
    manifest.insert(
        "assets".to_string(),
        serde_json::Value::Array(
            selected_assets
                .iter()
                .map(|(id, name, kind, sha256, byte_length)| {
                    serde_json::json!({
                        "id": id,
                        "name": name,
                        "kind": kind,
                        "sha256": sha256,
                        "byte_length": byte_length,
                    })
                })
                .collect(),
        ),
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
    let release_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let inserted = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id,
         manifest_sha256, create_request_sha256, state, release_notes)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'draft',$10)
         ON CONFLICT (repo_id, version) DO NOTHING
         RETURNING id",
    )
    .bind(release_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&body.version)
    .bind(&body.tag_name)
    .bind(&body.commit_oid)
    .bind(body.pipeline_run_id)
    .bind(&manifest_hex)
    .bind(&request_sha256)
    .bind(&body.release_notes)
    .fetch_optional(&mut *tx)
    .await;
    let inserted = match inserted {
        Ok(inserted) => inserted,
        Err(error)
            if error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .as_deref()
                == Some("23505") =>
        {
            tx.rollback().await?;
            return Err(AppError::Conflict(
                "release tag is already assigned to another release".to_string(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    if inserted.is_none() {
        tx.commit().await?;
        let Some((existing, existing_hash)) =
            find_release_by_version(&state.pool, repo_id, &body.version).await?
        else {
            return Err(AppError::Other(
                "release version conflict did not return the existing release".to_string(),
            ));
        };
        return if existing_hash.as_deref() == Some(request_sha256.as_str()) {
            Ok(existing)
        } else {
            Err(AppError::Conflict(
                "release version already exists with different request parameters".to_string(),
            ))
        };
    }
    for (artifact_id, _, _, _, _) in &selected_assets {
        sqlx::query("INSERT INTO release_artifacts (release_id, artifact_id) VALUES ($1, $2)")
            .bind(release_id)
            .bind(artifact_id)
            .execute(&mut *tx)
            .await?;
    }
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
        release_notes: body.release_notes,
        assets: selected_assets
            .into_iter()
            .map(
                |(id, name, kind, sha256, byte_length)| ReleaseArtifactView {
                    id,
                    name,
                    kind,
                    sha256,
                    byte_length,
                },
            )
            .collect(),
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
    crate::grants::ensure_repo_release_maintainer(&state.pool, org_id, repo_id, principal_id)
        .await?;
    let view = create_release_core(&state, org_id, repo_id, body).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn ensure_release_git_tag(
    state: &Arc<crate::AppState>,
    storage_key: Uuid,
    tag_name: &str,
    commit_oid: &str,
) -> Result<(), AppError> {
    validate_release_tag_name(tag_name).await?;
    let bare_repo = crate::git_http::bare_repo_path(&state.git_root, &storage_key);
    let tag_name = tag_name.to_string();
    let commit_oid = commit_oid.to_string();
    tokio::task::spawn_blocking(move || {
        let ref_name = format!("refs/tags/{tag_name}");
        let expected_oid = format!("{commit_oid}^{{commit}}");
        let commit = Command::new("git")
            .arg("--git-dir")
            .arg(&bare_repo)
            .args(["rev-parse", "--verify", &expected_oid])
            .output()
            .map_err(|error| AppError::Other(format!("resolve release commit: {error}")))?;
        if !commit.status.success() {
            return Err(AppError::Validation(
                "release commit is not present in the repository".to_string(),
            ));
        }
        let resolved_oid = String::from_utf8_lossy(&commit.stdout).trim().to_string();
        if !resolved_oid.eq_ignore_ascii_case(&commit_oid) {
            return Err(AppError::Validation(
                "release commit does not resolve to a commit object".to_string(),
            ));
        }

        let zero_oid = "0000000000000000000000000000000000000000";
        let update = Command::new("git")
            .arg("--git-dir")
            .arg(&bare_repo)
            .args(["update-ref", &ref_name, &resolved_oid, zero_oid])
            .output()
            .map_err(|error| AppError::Other(format!("create release Git tag: {error}")))?;
        if update.status.success() {
            return Ok(());
        }

        let existing = Command::new("git")
            .arg("--git-dir")
            .arg(&bare_repo)
            .args(["rev-parse", "--verify", &ref_name])
            .output()
            .map_err(|error| AppError::Other(format!("read existing release Git tag: {error}")))?;
        if existing.status.success()
            && String::from_utf8_lossy(&existing.stdout)
                .trim()
                .eq_ignore_ascii_case(&resolved_oid)
        {
            return Ok(());
        }
        Err(AppError::Conflict(
            "Git tag already exists and points to another object".to_string(),
        ))
    })
    .await
    .map_err(|error| AppError::Other(format!("release Git tag task failed: {error}")))?
}

pub async fn publish_release_core(
    state: &Arc<crate::AppState>,
    repo_id: Uuid,
    release_id: Uuid,
    body: PublishReleaseRequest,
) -> Result<ReleaseView, AppError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            Uuid,
            String,
            Option<String>,
            Option<String>,
            String,
        ),
    >(
        "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state,
         manifest_sha256, oci_image_ref, release_notes FROM releases WHERE id = $1 AND repo_id = $2",
    )
    .bind(release_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(AppError::NotFound);
    };
    let (
        id,
        version,
        tag_name,
        commit_oid,
        pipeline_run_id,
        release_state,
        old_manifest,
        old_oci,
        release_notes,
    ) = row;
    if release_state == "published" {
        if body
            .oci_image_ref
            .as_ref()
            .is_some_and(|submitted| old_oci.as_ref() != Some(submitted))
        {
            return Err(AppError::Conflict(
                "published release cannot change its OCI image reference".to_string(),
            ));
        }
        let storage_key: (Uuid,) =
            sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
                .bind(repo_id)
                .fetch_one(&state.pool)
                .await?;
        ensure_release_git_tag(state, storage_key.0, &tag_name, &commit_oid).await?;
        let assets = load_release_assets(&state.pool, id).await?;
        return Ok(ReleaseView {
            id,
            version,
            tag_name,
            commit_oid,
            pipeline_run_id,
            state: release_state,
            release_notes,
            assets,
            manifest_sha256: old_manifest,
            oci_image_ref: old_oci,
        });
    }
    if release_state != "draft" {
        return Err(AppError::Validation(
            "release cannot be published".to_string(),
        ));
    }

    artifacts::verify_ready_artifacts_for_pipeline(&state.pool, pipeline_run_id).await?;
    let assets = load_release_assets(&state.pool, id).await?;
    let asset_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM release_artifacts WHERE release_id = $1")
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    if assets.len() as i64 != asset_count.0 {
        return Err(AppError::Validation(
            "one or more selected release assets are no longer ready".to_string(),
        ));
    }
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
    manifest.insert("commit_oid".into(), commit_oid.clone().into());
    manifest.insert("pipeline_run_id".into(), pipeline_run_id.to_string().into());
    manifest.insert("release_notes".into(), release_notes.clone().into());
    manifest.insert(
        "assets".into(),
        serde_json::to_value(&assets).map_err(|error| AppError::Other(error.to_string()))?,
    );
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
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(repo_id)
        .fetch_one(&state.pool)
        .await?;
    ensure_release_git_tag(state, storage_key.0, &tag_name, &commit_oid).await?;

    let updated = sqlx::query(
        "UPDATE releases SET state = 'published', oci_image_ref = $3,
         manifest_sha256 = $4, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND state = 'draft'",
    )
    .bind(release_id)
    .bind(repo_id)
    .bind(&oci_to_set)
    .bind(&manifest_hex)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        let published = sqlx::query_as::<_, (String, String)>(
            "SELECT state, commit_oid FROM releases WHERE id = $1 AND repo_id = $2",
        )
        .bind(release_id)
        .bind(repo_id)
        .fetch_optional(&state.pool)
        .await?;
        if !matches!(published, Some((state, oid)) if state == "published" && oid == commit_oid) {
            return Err(AppError::Conflict(
                "release changed while it was being published".to_string(),
            ));
        }
    }
    let row = sqlx::query_as::<
        _,
        (
            Uuid, String, String, String, Uuid, String, Option<String>, Option<String>, String,
        ),
    >(
        "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state,
         manifest_sha256, oci_image_ref, release_notes FROM releases WHERE id = $1 AND repo_id = $2",
    )
    .bind(release_id)
    .bind(repo_id)
    .fetch_one(&state.pool)
    .await?;
    let assets = load_release_assets(&state.pool, row.0).await?;
    Ok(ReleaseView {
        id: row.0,
        version: row.1,
        tag_name: row.2,
        commit_oid: row.3,
        pipeline_run_id: row.4,
        state: row.5,
        release_notes: row.8,
        assets,
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
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, Uuid, String, Option<String>, Option<String>, String)>(
        "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256, oci_image_ref, release_notes
         FROM releases WHERE repo_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(repo_id)
    .fetch_all(&state.pool)
    .await?;
    let mut releases = Vec::with_capacity(rows.len());
    for row in rows {
        releases.push(ReleaseView {
            id: row.0,
            version: row.1,
            tag_name: row.2,
            commit_oid: row.3,
            pipeline_run_id: row.4,
            state: row.5,
            release_notes: row.8,
            assets: load_release_assets(&state.pool, row.0).await?,
            manifest_sha256: row.6,
            oci_image_ref: row.7,
        });
    }
    Ok(Json(releases))
}

pub async fn publish_release(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, release_id)): AxumPath<(String, String, Uuid)>,
    body: Option<Json<PublishReleaseRequest>>,
) -> Result<Json<ReleaseView>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    crate::grants::ensure_repo_release_maintainer(&state.pool, org_id, repo_id, principal_id)
        .await?;
    let view = publish_release_core(&state, repo_id, release_id, body).await?;
    Ok(Json(view))
}
