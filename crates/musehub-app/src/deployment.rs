//! Test environment deployment via compose-mock or isolated compose-runner.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::process::Command;
use uuid::Uuid;

use crate::compose::resolve_compose_adapter;
use crate::error::AppError;
use crate::security::{validate_compose_health_target, validate_oci_image_ref};

#[derive(Serialize)]
pub struct DeploymentView {
    pub id: Uuid,
    pub environment_slug: String,
    pub release_id: Uuid,
    pub state: String,
    pub health_summary: Option<serde_json::Value>,
    pub adapter_kind: String,
    pub attempt_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct StartDeploymentRequest {
    pub release_id: Uuid,
    pub environment_slug: Option<String>,
    pub health_target: Option<String>,
    /// Client-supplied idempotency key; replay returns the same deployment row.
    pub attempt_id: Option<Uuid>,
}

type DeploymentRow = (Uuid, Uuid, Uuid, String, String, Option<serde_json::Value>);

async fn fetch_by_attempt(
    pool: &PgPool,
    repo_id: Uuid,
    attempt_id: Uuid,
) -> Result<Option<DeploymentRow>, AppError> {
    sqlx::query_as::<_, DeploymentRow>(
        "SELECT d.id, d.release_id, d.environment_id, d.state, d.adapter_kind, d.health_summary
         FROM deployments d
         WHERE d.repo_id = $1 AND d.attempt_id = $2",
    )
    .bind(repo_id)
    .bind(attempt_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)
}

#[allow(clippy::too_many_arguments)]
fn row_to_view(
    id: Uuid,
    release_id: Uuid,
    env_slug: &str,
    dep_state: String,
    adapter_kind: String,
    health_summary: Option<serde_json::Value>,
    attempt_id: Uuid,
    status: StatusCode,
) -> (StatusCode, Json<DeploymentView>) {
    (
        status,
        Json(DeploymentView {
            id,
            environment_slug: env_slug.to_string(),
            release_id,
            state: dep_state,
            health_summary,
            adapter_kind,
            attempt_id: Some(attempt_id),
        }),
    )
}

pub async fn start_deployment(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Json(body): Json<StartDeploymentRequest>,
) -> Result<(StatusCode, Json<DeploymentView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    start_deployment_inner(&state, org_id, repo_id, body).await
}

pub(crate) async fn mcp_start_deployment(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    environment_id: Uuid,
    release_id: Uuid,
) -> Result<serde_json::Value, AppError> {
    let env = sqlx::query_as::<_, (String,)>(
        "SELECT slug FROM environments WHERE id = $1 AND repo_id = $2",
    )
    .bind(environment_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((env_slug,)) = env else {
        return Err(AppError::NotFound);
    };
    let body = StartDeploymentRequest {
        release_id,
        attempt_id: Some(Uuid::new_v4()),
        environment_slug: Some(env_slug),
        health_target: None,
    };
    let (status, Json(view)) = start_deployment_inner(state, org_id, repo_id, body).await?;
    Ok(serde_json::json!({
        "id": view.id.to_string(),
        "release_id": view.release_id.to_string(),
        "environment_slug": view.environment_slug,
        "state": view.state,
        "adapter_kind": view.adapter_kind,
        "attempt_id": view.attempt_id.map(|u| u.to_string()),
        "http_status": status.as_u16(),
    }))
}

async fn start_deployment_inner(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    body: StartDeploymentRequest,
) -> Result<(StatusCode, Json<DeploymentView>), AppError> {
    let env_slug = body
        .environment_slug
        .clone()
        .unwrap_or_else(|| "test".to_string());
    let env = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM environments WHERE repo_id = $1 AND slug = $2",
    )
    .bind(repo_id)
    .bind(&env_slug)
    .fetch_optional(&state.pool)
    .await?;
    let Some((environment_id,)) = env else {
        return Err(AppError::NotFound);
    };
    let _ = org_id;
    let release = sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT state, manifest_sha256, oci_image_ref, commit_oid FROM releases WHERE id = $1 AND repo_id = $2",
    )
    .bind(body.release_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((release_state, manifest, oci_image_ref, release_commit_oid)) = release else {
        return Err(AppError::NotFound);
    };
    if release_state != "published" {
        return Err(AppError::Validation(
            "deployment requires published release".to_string(),
        ));
    }

    let attempt_id = body.attempt_id.unwrap_or_else(Uuid::new_v4);

    if let Some((id, rel_id, env_id, dep_state, adapter_kind, health_summary)) =
        fetch_by_attempt(&state.pool, repo_id, attempt_id).await?
    {
        if rel_id != body.release_id {
            return Err(AppError::Conflict(
                "attempt_id already used for a different release".to_string(),
            ));
        }
        if env_id != environment_id {
            return Err(AppError::Conflict(
                "attempt_id already used for a different environment".to_string(),
            ));
        }
        return Ok(row_to_view(
            id,
            rel_id,
            &env_slug,
            dep_state,
            adapter_kind,
            health_summary,
            attempt_id,
            StatusCode::OK,
        ));
    }

    let pipeline_id: (Uuid,) = sqlx::query_as("SELECT pipeline_run_id FROM releases WHERE id = $1")
        .bind(body.release_id)
        .fetch_one(&state.pool)
        .await?;
    crate::artifacts::verify_ready_artifacts_for_pipeline(&state.pool, pipeline_id.0).await?;
    let deployment_id = Uuid::new_v4();
    let publish_port = 20_000u32 + (deployment_id.as_u128() % 15_000) as u32;
    let default_runner_target = format!("http://127.0.0.1:{publish_port}/");
    let health_target = match (body.health_target.clone(), state.require_pipeline_oci) {
        (Some(t), _) => t,
        (None, true) => default_runner_target.clone(),
        (None, false) => "mock://healthy".to_string(),
    };
    if state.require_pipeline_oci && health_target.starts_with("mock://") {
        return Err(AppError::Validation(
            "product OCI deploy rejects mock health target".to_string(),
        ));
    }
    validate_compose_health_target(&health_target).map_err(AppError::Validation)?;

    let (compose_bin, adapter_kind) = resolve_compose_adapter(
        &state.compose_adapter_bin,
        &state.compose_runner_bin,
        state.require_pipeline_oci,
    )
    .map_err(AppError::Validation)?;
    if adapter_kind == "compose-runner" {
        let Some(ref oci) = oci_image_ref else {
            return Err(AppError::Validation(
                "compose-runner deployment requires release.oci_image_ref".to_string(),
            ));
        };
        validate_oci_image_ref(oci).map_err(AppError::Validation)?;
    }
    let insert = sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id)
         VALUES ($1,$2,$3,$4,$5,$6,'running',$7)",
    )
    .bind(deployment_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(environment_id)
    .bind(body.release_id)
    .bind(adapter_kind)
    .bind(attempt_id)
    .execute(&state.pool)
    .await;

    if let Err(sqlx::Error::Database(db_err)) = &insert {
        if db_err.code().as_deref() == Some("23505") {
            if let Some(row) = fetch_by_attempt(&state.pool, repo_id, attempt_id).await? {
                let (id, rel_id, env_id, dep_state, kind, health_summary) = row;
                if rel_id != body.release_id || env_id != environment_id {
                    return Err(AppError::Conflict(
                        "attempt_id reused with different deployment payload".to_string(),
                    ));
                }
                return Ok(row_to_view(
                    id,
                    rel_id,
                    &env_slug,
                    dep_state,
                    kind,
                    health_summary,
                    attempt_id,
                    StatusCode::OK,
                ));
            }
        }
    }
    insert?;

    let project = format!("musehub-deploy-{deployment_id}");
    let mut cmd = Command::new(&compose_bin);
    cmd.env("MUSEHUB_DEPLOY_HEALTH_TARGET", &health_target)
        .env(
            "MUSEHUB_RELEASE_MANIFEST_SHA256",
            manifest.clone().unwrap_or_default(),
        )
        .env("MUSEHUB_COMPOSE_PROJECT", &project)
        .env("MUSEHUB_COMPOSE_PUBLISH_PORT", publish_port.to_string())
        .env("MUSEHUB_RELEASE_COMMIT_OID", &release_commit_oid)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(ref oci) = oci_image_ref {
        cmd.env("MUSEHUB_OCI_IMAGE_REF", oci);
    }
    let child = cmd.spawn().map_err(|e| AppError::Other(e.to_string()))?;
    let finished = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output()).await;
    let (ok, adapter_stdout, capability_gap) = match &finished {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let cap = out.status.code() == Some(2);
            (out.status.success(), stdout, cap)
        }
        _ => (false, String::new(), false),
    };
    let applied_digest = adapter_stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|v| {
            v.get("applied_oci_digest")
                .and_then(|d| d.as_str())
                .map(String::from)
        });
    let verified_source_oid = adapter_stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|v| {
            v.get("verified_source_oid")
                .and_then(|d| d.as_str())
                .map(String::from)
        });
    let digest_ok = if adapter_kind == "compose-runner" {
        match (oci_image_ref.as_ref(), applied_digest.as_ref()) {
            (Some(req), Some(app)) => req.split('@').nth(1) == Some(app.as_str()),
            (Some(_), None) => false,
            _ => false,
        }
    } else {
        true
    };
    let source_ok = if adapter_kind == "compose-runner" {
        verified_source_oid.as_deref() == Some(release_commit_oid.as_str())
    } else {
        true
    };
    let ok = ok && digest_ok && source_ok;
    let summary = if ok {
        serde_json::json!({
            "status": "healthy",
            "target": health_target,
            "adapter": adapter_kind,
            "release_manifest_sha256": manifest,
            "release_oci_image_ref": oci_image_ref,
            "applied_oci_digest": applied_digest,
            "verified_source_oid": verified_source_oid,
            "pipeline_source_oid": release_commit_oid,
            "compose_project": project,
        })
    } else if capability_gap {
        serde_json::json!({
            "status": "unhealthy",
            "adapter": adapter_kind,
            "error_code": "docker_unavailable",
            "compose_project": project,
        })
    } else {
        let err = if !digest_ok {
            "oci_digest_mismatch"
        } else if !source_ok {
            "source_oid_mismatch"
        } else {
            "deploy_failed"
        };
        serde_json::json!({
            "status": "unhealthy",
            "adapter": adapter_kind,
            "error_code": err,
            "release_oci_image_ref": oci_image_ref,
            "applied_oci_digest": applied_digest,
            "verified_source_oid": verified_source_oid,
            "pipeline_source_oid": release_commit_oid,
            "compose_project": project,
        })
    };
    let state_name = if ok { "succeeded" } else { "failed" };
    sqlx::query(
        "UPDATE deployments SET state = $2, health_summary = $3, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(deployment_id)
    .bind(state_name)
    .bind(summary.clone())
    .execute(&state.pool)
    .await?;

    Ok(row_to_view(
        deployment_id,
        body.release_id,
        &env_slug,
        state_name.to_string(),
        adapter_kind.to_string(),
        Some(summary),
        attempt_id,
        StatusCode::CREATED,
    ))
}
