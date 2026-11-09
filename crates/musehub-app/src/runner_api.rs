//! Remote pipeline runner HTTP API (PAT auth, no Docker socket on control plane).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::grants;
use crate::lease::{self, WorkLease};
use crate::pat_auth::{self, PatContext};
use crate::AppState;

const RUNNER_ID_HEADER: &str = "x-musehub-runner-id";

pub fn pipeline_executor_remote() -> bool {
    matches!(
        std::env::var("MUSEHUB_PIPELINE_EXECUTOR").as_deref(),
        Ok("remote")
    )
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AppError> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .ok_or(AppError::Unauthorized)?;
    Ok(token.trim())
}

fn runner_instance_id(headers: &HeaderMap) -> Result<Uuid, AppError> {
    let raw = headers
        .get(RUNNER_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Validation("x-musehub-runner-id required".to_string()))?;
    Uuid::parse_str(raw.trim()).map_err(|_| AppError::Validation("invalid runner id".to_string()))
}

async fn require_runner_pat(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<PatContext, AppError> {
    let token = bearer_token(headers)?;
    let pat = pat_auth::authenticate_pat_context(&state.pool, token).await?;
    if !pat_auth::scope_allows(&pat.scopes, "pipeline:execute")
        && !pat_auth::scope_allows(&pat.scopes, "checks:write")
    {
        return Err(AppError::Forbidden);
    }
    if pat.org_id.is_none() {
        return Err(AppError::Validation(
            "runner tokens require org binding".to_string(),
        ));
    }
    Ok(pat)
}

#[derive(Serialize)]
pub struct RunnerCapabilities {
    pub pipeline_executor: &'static str,
    pub lease_seconds: i64,
    pub scopes_accepted: &'static [&'static str],
    pub note: &'static str,
}

pub async fn capabilities() -> Json<RunnerCapabilities> {
    Json(RunnerCapabilities {
        pipeline_executor: if pipeline_executor_remote() {
            "remote"
        } else {
            "inprocess"
        },
        lease_seconds: lease::LEASE_SECS,
        scopes_accepted: &["pipeline:execute", "checks:write"],
        note: "Control plane does not mount a Docker socket; workers execute adapters locally.",
    })
}

#[derive(Serialize, Deserialize)]
pub struct ClaimedPipelineRun {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub org_id: Uuid,
    pub org_slug: String,
    pub repo_id: Uuid,
    pub repo_slug: String,
    pub storage_key: Uuid,
    pub source_oid: String,
    pub run_kind: String,
    pub lease_epoch: i64,
}

pub async fn claim_pipeline_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ClaimedPipelineRun>), AppError> {
    if !pipeline_executor_remote() {
        return Err(AppError::Conflict(
            "pipeline executor is in-process on this server".to_string(),
        ));
    }
    let pat = require_runner_pat(&state, &headers).await?;
    let instance_id = runner_instance_id(&headers)?;
    let org_id = pat.org_id.unwrap();
    let candidate = sqlx::query_as::<_, (Uuid,)>(
        "SELECT pr.id FROM pipeline_runs pr
         WHERE pr.org_id = $1 AND (
           (pr.state = 'queued'
             AND (pr.lease_holder IS NULL OR pr.lease_until < CURRENT_TIMESTAMP))
           OR (pr.state IN ('running','cancel_requested')
             AND pr.lease_until < CURRENT_TIMESTAMP)
         )
         ORDER BY pr.created_at
         LIMIT 1",
    )
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((run_id,)) = candidate else {
        return Err(AppError::NotFound);
    };
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, Uuid)>(
        "SELECT pr.org_id, pr.repo_id, pr.source_oid, pr.run_kind, pj.id
         FROM pipeline_runs pr
         JOIN pipeline_jobs pj ON pj.pipeline_run_id = pr.id AND pj.job_key = 'build'
         WHERE pr.id = $1",
    )
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((row_org, repo_id, source_oid, run_kind, job_id)) = row else {
        return Err(AppError::NotFound);
    };
    grants::ensure_pat_repo_access(&state.pool, &pat, row_org, repo_id).await?;
    let work = lease::try_begin_pipeline_work(&state.pool, run_id, instance_id).await?;
    let meta = sqlx::query_as::<_, (String, String, Uuid)>(
        "SELECT o.slug, r.slug, r.storage_key
         FROM repositories r
         JOIN organizations o ON o.id = r.org_id
         WHERE r.id = $1",
    )
    .bind(repo_id)
    .fetch_one(&state.pool)
    .await?;
    Ok((
        StatusCode::OK,
        Json(ClaimedPipelineRun {
            run_id,
            job_id,
            org_id: row_org,
            org_slug: meta.0,
            repo_id,
            repo_slug: meta.1,
            storage_key: meta.2,
            source_oid,
            run_kind,
            lease_epoch: work.epoch,
        }),
    ))
}

#[derive(Deserialize)]
pub struct RenewBody {
    pub lease_epoch: i64,
}

pub async fn renew_pipeline_lease(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Json(body): Json<RenewBody>,
) -> Result<StatusCode, AppError> {
    let pat = require_runner_pat(&state, &headers).await?;
    let row = sqlx::query_as::<_, (Uuid,)>(
        "SELECT repo_id FROM pipeline_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some((repo_id,)) = row {
        let org_id = sqlx::query_as::<_, (Uuid,)>(
            "SELECT org_id FROM pipeline_runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_one(&state.pool)
        .await?
        .0;
        grants::ensure_pat_repo_access(&state.pool, &pat, org_id, repo_id).await?;
    }
    let instance_id = runner_instance_id(&headers)?;
    lease::renew_pipeline_work_lease(&state.pool, run_id, instance_id, body.lease_epoch).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_runner_tokens_masked(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let principal_id = crate::require_session(&state.pool, &headers).await?;
    let org_id = crate::org_id_for_member(&state.pool, &org, principal_id).await?;
    let is_privileged = grants::is_org_privileged(&state.pool, org_id, principal_id).await?;
    if !is_privileged {
        return Err(AppError::Forbidden);
    }
    let rows = sqlx::query_as::<_, (Uuid, String, String, Vec<String>, String)>(
        "SELECT id, name, token_prefix, scopes, scope_mode FROM access_tokens
         WHERE org_id = $1 AND revoked_at IS NULL
           AND (scopes @> ARRAY['checks:write']::text[] OR scopes @> ARRAY['pipeline:execute']::text[])
         ORDER BY created_at DESC",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, name, prefix, scopes, scope_mode)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "token_prefix": prefix,
                    "scopes": scopes,
                    "scope_mode": scope_mode,
                })
            })
            .collect(),
    ))
}

pub fn parse_runner_instance_from_env() -> Uuid {
    std::env::var("MUSEHUB_RUNNER_INSTANCE_ID")
        .ok()
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(Uuid::new_v4)
}

#[allow(dead_code)]
pub struct HeldLease {
    pub epoch: i64,
    pub instance_id: Uuid,
    pub run_id: Uuid,
}

impl HeldLease {
    pub fn from_claim(instance_id: Uuid, run_id: Uuid, lease: WorkLease) -> Self {
        Self {
            epoch: lease.epoch,
            instance_id,
            run_id,
        }
    }
}
