//! Test environment deployment via controlled compose-mock adapter (no Docker socket).

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::error::AppError;
use crate::security::validate_compose_health_target;

#[derive(Serialize)]
pub struct DeploymentView {
    pub id: Uuid,
    pub environment_slug: String,
    pub release_id: Uuid,
    pub state: String,
    pub health_summary: Option<serde_json::Value>,
    pub adapter_kind: String,
}

#[derive(Deserialize)]
pub struct StartDeploymentRequest {
    pub release_id: Uuid,
    pub environment_slug: Option<String>,
    pub health_target: Option<String>,
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
    let release = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, manifest_sha256 FROM releases WHERE id = $1 AND repo_id = $2",
    )
    .bind(body.release_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((release_state, manifest)) = release else {
        return Err(AppError::NotFound);
    };
    if release_state != "published" {
        return Err(AppError::Validation(
            "deployment requires published release".to_string(),
        ));
    }
    let health_target = body
        .health_target
        .clone()
        .unwrap_or_else(|| "mock://healthy".to_string());
    validate_compose_health_target(&health_target).map_err(AppError::Validation)?;
    let deployment_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state)
         VALUES ($1,$2,$3,$4,$5,'compose-mock','running')",
    )
    .bind(deployment_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(environment_id)
    .bind(body.release_id)
    .execute(&state.pool)
    .await?;

    let mut child = Command::new(&state.compose_adapter_bin)
        .env("MUSEHUB_DEPLOY_HEALTH_TARGET", &health_target)
        .env(
            "MUSEHUB_RELEASE_MANIFEST_SHA256",
            manifest.clone().unwrap_or_default(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Other(e.to_string()))?;
    let finished = tokio::time::timeout(Duration::from_secs(30), child.wait()).await;
    let ok = matches!(finished, Ok(Ok(status)) if status.success());
    let summary = if ok {
        serde_json::json!({
            "status": "healthy",
            "target": health_target,
            "adapter": "compose-mock",
            "release_manifest_sha256": manifest,
        })
    } else {
        serde_json::json!({
            "status": "unhealthy",
            "adapter": "compose-mock",
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

    Ok((
        StatusCode::CREATED,
        Json(DeploymentView {
            id: deployment_id,
            environment_slug: env_slug,
            release_id: body.release_id,
            state: state_name.to_string(),
            health_summary: Some(summary),
            adapter_kind: "compose-mock".to_string(),
        }),
    ))
}
