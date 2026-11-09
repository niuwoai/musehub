//! Reverse trace: deployment → release → pipeline → PR → issue → agent run.

use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;

#[derive(Serialize)]
pub struct TraceView {
    pub deployment_id: Uuid,
    pub release_id: Uuid,
    pub pipeline_run_id: Uuid,
    pub commit_oid: String,
    pub pr_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub artifacts: Vec<TraceArtifact>,
}

#[derive(Serialize)]
pub struct TraceArtifact {
    pub name: String,
    pub sha256: String,
    pub byte_length: i64,
}

pub async fn get_deployment_trace(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, deployment_id)): AxumPath<(String, String, Uuid)>,
) -> Result<Json<TraceView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let row = sqlx::query_as::<_, (Uuid,)>(
        "SELECT release_id FROM deployments WHERE id = $1 AND repo_id = $2",
    )
    .bind(deployment_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((release_id,)) = row else {
        return Err(AppError::NotFound);
    };
    let release = sqlx::query_as::<_, (String, Uuid)>(
        "SELECT commit_oid, pipeline_run_id FROM releases WHERE id = $1",
    )
    .bind(release_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| AppError::NotFound)?;
    let (commit_oid, pipeline_run_id) = release;
    let pipe = sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>, Option<Uuid>)>(
        "SELECT pr_id, issue_id, agent_run_id FROM pipeline_runs WHERE id = $1",
    )
    .bind(pipeline_run_id)
    .fetch_one(&state.pool)
    .await?;
    let pr_from_link: Option<Uuid> = sqlx::query_as::<_, (Uuid,)>(
        "SELECT pr_id FROM release_pr_links WHERE release_id = $1 LIMIT 1",
    )
    .bind(release_id)
    .fetch_optional(&state.pool)
    .await?
    .map(|(id,)| id);
    let artifacts = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT name, sha256, byte_length FROM artifacts
         WHERE pipeline_run_id = $1 AND state = 'ready' ORDER BY name",
    )
    .bind(pipeline_run_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(TraceView {
        deployment_id,
        release_id,
        pipeline_run_id,
        commit_oid,
        pr_id: pipe.0.or(pr_from_link),
        issue_id: pipe.1,
        agent_run_id: pipe.2,
        artifacts: artifacts
            .into_iter()
            .map(|(name, sha256, byte_length)| TraceArtifact {
                name,
                sha256,
                byte_length,
            })
            .collect(),
    }))
}
