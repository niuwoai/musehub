//! Test environment deployment via compose-mock or isolated compose-runner.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::process::Command;
use uuid::Uuid;

use std::path::PathBuf;

use crate::compose::resolve_compose_adapter;
use crate::error::AppError;
use crate::runner_api;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
    pub approval_required: bool,
}

#[derive(Deserialize)]
pub struct DeploymentApprovalRequest {
    pub expected_plan_hash: String,
    pub decision: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct DeploymentSummary {
    pub id: Uuid,
    pub environment_slug: String,
    pub release_id: Uuid,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
    pub approval_required: bool,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct DeploymentListResponse {
    pub items: Vec<DeploymentSummary>,
    pub has_more: bool,
}

#[derive(Deserialize, Default)]
pub struct ListDeploymentsQuery {
    pub limit: Option<i64>,
    pub state: Option<String>,
    pub environment_slug: Option<String>,
}

#[derive(Deserialize)]
pub struct StartDeploymentRequest {
    pub release_id: Uuid,
    pub environment_slug: Option<String>,
    pub health_target: Option<String>,
    /// Client-supplied idempotency key; replay returns the same deployment row.
    pub attempt_id: Option<Uuid>,
}

type DeploymentRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    Option<serde_json::Value>,
    Option<Uuid>,
);

struct DeploymentLaunch {
    release_id: Uuid,
    environment_slug: String,
    health_target: Option<String>,
    attempt_id: Uuid,
    rollback_of_id: Option<Uuid>,
    rollback_reason: Option<String>,
    audit_action: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DeploymentLaunchPayload {
    pub health_target: String,
    pub publish_port: u32,
    pub project: String,
    pub teardown_project: Option<String>,
    pub product_runner: bool,
    pub release_commit_oid: String,
    pub manifest: Option<String>,
    pub oci_image_ref: Option<String>,
    pub adapter_kind: String,
    pub rollback_of_id: Option<Uuid>,
    pub rollback_reason: Option<String>,
    pub audit_action: String,
    pub environment_slug: String,
    pub actor_id: Option<String>,
}

async fn fetch_by_attempt(
    pool: &PgPool,
    repo_id: Uuid,
    attempt_id: Uuid,
) -> Result<Option<DeploymentRow>, AppError> {
    sqlx::query_as::<_, DeploymentRow>(
        "SELECT d.id, d.release_id, d.environment_id, d.state, d.adapter_kind, d.health_summary, d.rollback_of_id
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
    plan_hash: Option<String>,
    status: StatusCode,
) -> (StatusCode, Json<DeploymentView>) {
    (
        status,
        Json(DeploymentView {
            id,
            environment_slug: env_slug.to_string(),
            release_id,
            state: dep_state.clone(),
            health_summary,
            adapter_kind,
            attempt_id: Some(attempt_id),
            plan_hash,
            approval_required: dep_state == "awaiting_approval",
        }),
    )
}

pub async fn list_deployments(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Query(query): Query<ListDeploymentsQuery>,
) -> Result<Json<DeploymentListResponse>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    let fetch = limit + 1;
    let mut sql = String::from(
        "SELECT d.id, e.slug, d.release_id, d.state, d.plan_hash::text, d.updated_at
         FROM deployments d
         JOIN environments e ON e.id = d.environment_id
         WHERE d.repo_id = $1",
    );
    let mut bind_idx = 2i32;
    let env_slug = query
        .environment_slug
        .as_ref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let state_filter = query
        .state
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if env_slug.is_some() {
        sql.push_str(&format!(" AND e.slug = ${bind_idx}"));
        bind_idx += 1;
    }
    if state_filter.is_some() {
        sql.push_str(&format!(" AND d.state = ${bind_idx}"));
        bind_idx += 1;
    }
    sql.push_str(&format!(
        " ORDER BY d.created_at DESC, d.id DESC LIMIT ${bind_idx}"
    ));

    let mut q = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Uuid,
            String,
            Option<String>,
            time::OffsetDateTime,
        ),
    >(&sql)
    .bind(repo_id);
    if let Some(ref slug) = env_slug {
        q = q.bind(slug);
    }
    if let Some(ref st) = state_filter {
        q = q.bind(st);
    }
    q = q.bind(fetch);
    let rows = q.fetch_all(&state.pool).await?;
    let has_more = rows.len() as i64 > limit;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(
            |(id, env_slug, release_id, dep_state, plan_hash, updated_at)| DeploymentSummary {
                id,
                environment_slug: env_slug,
                release_id,
                state: dep_state.clone(),
                plan_hash,
                approval_required: dep_state == "awaiting_approval",
                updated_at: updated_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| updated_at.to_string()),
            },
        )
        .collect();
    Ok(Json(DeploymentListResponse { items, has_more }))
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
    let launch = DeploymentLaunch {
        release_id: body.release_id,
        environment_slug: body
            .environment_slug
            .clone()
            .unwrap_or_else(|| "test".to_string()),
        health_target: body.health_target.clone(),
        attempt_id: body.attempt_id.unwrap_or_else(Uuid::new_v4),
        rollback_of_id: None,
        rollback_reason: None,
        audit_action: "deployment.start".to_string(),
    };
    launch_deployment(
        &state,
        org_id,
        repo_id,
        Some(principal_id),
        crate::audit::request_id_from_headers(&headers),
        launch,
    )
    .await
}

pub async fn get_deployment(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, deployment_id)): AxumPath<(String, String, Uuid)>,
) -> Result<Json<DeploymentView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            String,
            String,
            Option<serde_json::Value>,
            Option<Uuid>,
        ),
    >(
        "SELECT d.id, d.release_id, e.slug, d.state, d.adapter_kind, d.health_summary, d.attempt_id
         FROM deployments d
         JOIN environments e ON e.id = d.environment_id
         WHERE d.id = $1 AND d.repo_id = $2",
    )
    .bind(deployment_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, release_id, env_slug, dep_state, adapter_kind, health_summary, attempt_id)) = row
    else {
        return Err(AppError::NotFound);
    };
    let plan_hash: Option<String> =
        sqlx::query_scalar("SELECT plan_hash::text FROM deployments WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    Ok(Json(DeploymentView {
        id,
        environment_slug: env_slug,
        release_id,
        state: dep_state.clone(),
        health_summary,
        adapter_kind,
        attempt_id,
        plan_hash,
        approval_required: dep_state == "awaiting_approval",
    }))
}

pub async fn approve_deployment(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, deployment_id)): AxumPath<(String, String, Uuid)>,
    Json(body): Json<DeploymentApprovalRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let result = apply_deployment_approval(
        &state,
        org_id,
        repo_id,
        principal_id,
        deployment_id,
        &body.expected_plan_hash,
        &body.decision,
        &body.reason,
    )
    .await?;
    Ok(Json(result))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn mcp_rollback_deployment(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    environment_id: Uuid,
    target_deployment_id: Uuid,
    attempt_id: Uuid,
    reason: Option<String>,
    actor_id: Option<Uuid>,
    request_id: Option<Uuid>,
) -> Result<(StatusCode, Json<DeploymentView>), AppError> {
    let target = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Option<serde_json::Value>)>(
        "SELECT d.id, d.environment_id, d.release_id, d.state, d.health_summary
         FROM deployments d WHERE d.id = $1 AND d.repo_id = $2",
    )
    .bind(target_deployment_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((target_id, env_id, release_id, dep_state, health_summary)) = target else {
        return Err(AppError::NotFound);
    };
    if env_id != environment_id {
        return Err(AppError::Validation(
            "target deployment is not in the requested environment".to_string(),
        ));
    }
    if dep_state != "succeeded" {
        return Err(AppError::Validation(
            "rollback target must be a succeeded deployment".to_string(),
        ));
    }
    let env_slug: (String,) = sqlx::query_as(
        "SELECT e.slug FROM deployments d JOIN environments e ON e.id = d.environment_id WHERE d.id = $1",
    )
    .bind(target_id)
    .fetch_one(&state.pool)
    .await?;
    let health_target = if state.require_pipeline_oci {
        None
    } else {
        health_summary
            .as_ref()
            .and_then(|v| v.get("target"))
            .and_then(|t| t.as_str())
            .map(String::from)
    };
    let launch = DeploymentLaunch {
        release_id,
        environment_slug: env_slug.0.clone(),
        health_target,
        attempt_id,
        rollback_of_id: Some(target_id),
        rollback_reason: reason,
        audit_action: "deployment.rollback".to_string(),
    };
    launch_deployment(state, org_id, repo_id, actor_id, request_id, launch).await
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
    let launch = DeploymentLaunch {
        release_id,
        environment_slug: env_slug,
        health_target: None,
        attempt_id: Uuid::new_v4(),
        rollback_of_id: None,
        rollback_reason: None,
        audit_action: "deployment.start".to_string(),
    };
    let pat = state.mcp_pat_slot.read().await;
    let actor = pat.as_ref().map(|p| p.principal_id);
    drop(pat);
    let (status, Json(view)) =
        launch_deployment(state, org_id, repo_id, actor, None, launch).await?;
    let plan_hash: Option<String> =
        sqlx::query_scalar("SELECT plan_hash::text FROM deployments WHERE id = $1")
            .bind(view.id)
            .fetch_optional(&state.pool)
            .await?;
    Ok(serde_json::json!({
        "id": view.id.to_string(),
        "release_id": view.release_id.to_string(),
        "environment_slug": view.environment_slug,
        "state": view.state,
        "adapter_kind": view.adapter_kind,
        "attempt_id": view.attempt_id.map(|u| u.to_string()),
        "http_status": status.as_u16(),
        "approval_required": view.state == "awaiting_approval",
        "plan_hash": plan_hash,
    }))
}

async fn launch_deployment(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    actor_id: Option<Uuid>,
    request_id: Option<Uuid>,
    launch: DeploymentLaunch,
) -> Result<(StatusCode, Json<DeploymentView>), AppError> {
    let env_slug = launch.environment_slug.clone();
    let env = sqlx::query_as::<_, (Uuid, bool)>(
        "SELECT id, requires_approval FROM environments WHERE repo_id = $1 AND slug = $2",
    )
    .bind(repo_id)
    .bind(&env_slug)
    .fetch_optional(&state.pool)
    .await?;
    let Some((environment_id, requires_approval)) = env else {
        return Err(AppError::NotFound);
    };
    let _ = org_id;
    let release = sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT state, manifest_sha256, oci_image_ref, commit_oid FROM releases WHERE id = $1 AND repo_id = $2",
    )
    .bind(launch.release_id)
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

    let attempt_id = launch.attempt_id;

    if let Some((id, rel_id, env_id, dep_state, adapter_kind, health_summary, stored_rollback)) =
        fetch_by_attempt(&state.pool, repo_id, attempt_id).await?
    {
        if rel_id != launch.release_id {
            return Err(AppError::Conflict(
                "attempt_id already used for a different release".to_string(),
            ));
        }
        if env_id != environment_id {
            return Err(AppError::Conflict(
                "attempt_id already used for a different environment".to_string(),
            ));
        }
        if launch.rollback_of_id.is_some() && stored_rollback != launch.rollback_of_id {
            return Err(AppError::Conflict(
                "attempt_id already used for a different rollback target".to_string(),
            ));
        }
        if launch.rollback_of_id.is_none() && stored_rollback.is_some() {
            return Err(AppError::Conflict(
                "attempt_id already used for a rollback deployment".to_string(),
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
            None,
            StatusCode::OK,
        ));
    }

    let pipeline_id: (Uuid,) = sqlx::query_as("SELECT pipeline_run_id FROM releases WHERE id = $1")
        .bind(launch.release_id)
        .fetch_one(&state.pool)
        .await?;
    crate::artifacts::verify_ready_artifacts_for_pipeline(&state.pool, pipeline_id.0).await?;
    let deployment_id = Uuid::new_v4();
    let publish_port = 20_000u32 + (deployment_id.as_u128() % 15_000) as u32;
    let default_runner_target = format!("http://127.0.0.1:{publish_port}/");
    let health_target = match (launch.health_target.clone(), state.require_pipeline_oci) {
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
    let product_runner = state.require_pipeline_oci && adapter_kind == "compose-runner";
    let teardown_project: Option<String> = if product_runner || launch.rollback_of_id.is_some() {
        sqlx::query_scalar(
            "SELECT d.health_summary->>'compose_project' FROM deployments d
             WHERE d.repo_id = $1 AND d.environment_id = $2 AND d.state = 'succeeded' AND d.id != $3
             ORDER BY d.updated_at DESC NULLS LAST LIMIT 1",
        )
        .bind(repo_id)
        .bind(environment_id)
        .bind(deployment_id)
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };
    let remote_deploy = runner_api::deploy_executor_remote();
    let payload = DeploymentLaunchPayload {
        health_target: health_target.clone(),
        publish_port,
        project: String::new(),
        teardown_project: None,
        product_runner,
        release_commit_oid: release_commit_oid.clone(),
        manifest: manifest.clone(),
        oci_image_ref: oci_image_ref.clone(),
        adapter_kind: adapter_kind.to_string(),
        rollback_of_id: launch.rollback_of_id,
        rollback_reason: launch.rollback_reason.clone(),
        audit_action: launch.audit_action.clone(),
        environment_slug: env_slug.clone(),
        actor_id: actor_id.map(|u| u.to_string()),
    };
    let mut payload = payload;
    payload.project = if product_runner || launch.rollback_of_id.is_some() {
        format!("musehub-env-{environment_id}")
    } else {
        format!("musehub-deploy-{deployment_id}")
    };
    if let Some(ref prev) = teardown_project {
        payload.teardown_project = Some(prev.clone());
    }
    let plan_hash = manifest
        .as_deref()
        .filter(|m| m.len() == 64)
        .map(String::from);
    if requires_approval && plan_hash.is_none() {
        return Err(AppError::Validation(
            "protected environment requires published release manifest_sha256".to_string(),
        ));
    }
    let payload_json =
        serde_json::to_value(&payload).map_err(|e| AppError::Other(e.to_string()))?;
    let initial_state = if requires_approval {
        "awaiting_approval"
    } else if remote_deploy {
        "queued"
    } else {
        "running"
    };
    let insert = if requires_approval || remote_deploy {
        sqlx::query(
            "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id, rollback_of_id, launch_payload, plan_hash, requested_by)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(deployment_id)
        .bind(org_id)
        .bind(repo_id)
        .bind(environment_id)
        .bind(launch.release_id)
        .bind(adapter_kind)
        .bind(initial_state)
        .bind(attempt_id)
        .bind(launch.rollback_of_id)
        .bind(payload_json)
        .bind(plan_hash.as_deref())
        .bind(actor_id)
        .execute(&state.pool)
        .await
    } else {
        sqlx::query(
            "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id, rollback_of_id, plan_hash, requested_by)
             VALUES ($1,$2,$3,$4,$5,$6,'running',$7,$8,$9,$10)",
        )
        .bind(deployment_id)
        .bind(org_id)
        .bind(repo_id)
        .bind(environment_id)
        .bind(launch.release_id)
        .bind(adapter_kind)
        .bind(attempt_id)
        .bind(launch.rollback_of_id)
        .bind(plan_hash.as_deref())
        .bind(actor_id)
        .execute(&state.pool)
        .await
    };

    if let Err(sqlx::Error::Database(db_err)) = &insert {
        if db_err.code().as_deref() == Some("23505") {
            if let Some(row) = fetch_by_attempt(&state.pool, repo_id, attempt_id).await? {
                let (id, rel_id, env_id, dep_state, kind, health_summary, stored_rollback) = row;
                if rel_id != launch.release_id
                    || env_id != environment_id
                    || stored_rollback != launch.rollback_of_id
                {
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
                    None,
                    StatusCode::OK,
                ));
            }
        }
    }
    insert?;

    if requires_approval {
        if let Some(actor) = actor_id {
            let _ = crate::audit::record(
                &state.pool,
                Some(org_id),
                Some(repo_id),
                actor,
                None,
                "deployment.awaiting_approval",
                "deployment",
                deployment_id,
                request_id,
                "allow",
                serde_json::json!({
                    "release_id": launch.release_id.to_string(),
                    "plan_hash": plan_hash,
                    "environment_slug": env_slug,
                }),
            )
            .await;
        }
        return Ok(row_to_view(
            deployment_id,
            launch.release_id,
            &env_slug,
            "awaiting_approval".to_string(),
            adapter_kind.to_string(),
            None,
            attempt_id,
            plan_hash.clone(),
            StatusCode::ACCEPTED,
        ));
    }

    if remote_deploy {
        return Ok(row_to_view(
            deployment_id,
            launch.release_id,
            &env_slug,
            "queued".to_string(),
            adapter_kind.to_string(),
            None,
            attempt_id,
            None,
            StatusCode::ACCEPTED,
        ));
    }

    run_local_compose_deploy(
        state,
        deployment_id,
        org_id,
        repo_id,
        &launch,
        &env_slug,
        attempt_id,
        actor_id,
        request_id,
        &payload,
        &compose_bin,
        adapter_kind,
        &health_target,
        publish_port,
        &manifest,
        &oci_image_ref,
        &release_commit_oid,
        product_runner,
        &teardown_project,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_local_compose_deploy(
    state: &Arc<crate::AppState>,
    deployment_id: Uuid,
    org_id: Uuid,
    repo_id: Uuid,
    launch: &DeploymentLaunch,
    env_slug: &str,
    attempt_id: Uuid,
    actor_id: Option<Uuid>,
    request_id: Option<Uuid>,
    payload: &DeploymentLaunchPayload,
    compose_bin: &std::path::Path,
    adapter_kind: &str,
    health_target: &str,
    publish_port: u32,
    manifest: &Option<String>,
    oci_image_ref: &Option<String>,
    release_commit_oid: &str,
    product_runner: bool,
    teardown_project: &Option<String>,
) -> Result<(StatusCode, Json<DeploymentView>), AppError> {
    let project = payload.project.clone();
    let mut cmd = Command::new(compose_bin);
    cmd.env("MUSEHUB_DEPLOY_HEALTH_TARGET", health_target)
        .env(
            "MUSEHUB_RELEASE_MANIFEST_SHA256",
            manifest.clone().unwrap_or_default(),
        )
        .env("MUSEHUB_COMPOSE_PROJECT", &project)
        .env("MUSEHUB_COMPOSE_PUBLISH_PORT", publish_port.to_string())
        .env("MUSEHUB_RELEASE_COMMIT_OID", release_commit_oid)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(oci) = oci_image_ref {
        cmd.env("MUSEHUB_OCI_IMAGE_REF", oci);
    }
    if let Some(prev) = teardown_project {
        cmd.env("MUSEHUB_COMPOSE_TEARDOWN_PROJECT", prev);
    }
    if product_runner || (launch.rollback_of_id.is_some() && adapter_kind == "compose-runner") {
        cmd.env("MUSEHUB_COMPOSE_LEAVE_STACK", "1");
    }
    let child = cmd
        .spawn()
        .map_err(|e| AppError::Other(format!("spawn {compose_bin:?}: {e}")))?;
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
        verified_source_oid.as_deref() == Some(release_commit_oid)
    } else {
        true
    };
    let ok = ok && digest_ok && source_ok;
    let mut summary = if ok {
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
    if let Some(obj) = summary.as_object_mut() {
        if let Some(target_id) = launch.rollback_of_id {
            obj.insert("rollback".into(), serde_json::json!(true));
            obj.insert(
                "restored_from".into(),
                serde_json::json!(target_id.to_string()),
            );
            if let Some(reason) = launch.rollback_reason.as_ref() {
                obj.insert("reason".into(), serde_json::json!(reason));
            }
        }
    }
    let state_name = if ok { "succeeded" } else { "failed" };
    sqlx::query(
        "UPDATE deployments SET state = $2, health_summary = $3, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(deployment_id)
    .bind(state_name)
    .bind(summary.clone())
    .execute(&state.pool)
    .await?;

    if let Some(actor) = actor_id {
        let _ = crate::audit::record(
            &state.pool,
            Some(org_id),
            Some(repo_id),
            actor,
            None,
            launch.audit_action.as_str(),
            "deployment",
            deployment_id,
            request_id,
            if ok { "allow" } else { "deny" },
            serde_json::json!({
                "state": state_name,
                "adapter_kind": adapter_kind,
                "release_id": launch.release_id.to_string(),
                "rollback_of_id": launch.rollback_of_id.map(|u| u.to_string()),
            }),
        )
        .await;
    }

    Ok(row_to_view(
        deployment_id,
        launch.release_id,
        env_slug,
        state_name.to_string(),
        adapter_kind.to_string(),
        Some(summary),
        attempt_id,
        None,
        StatusCode::CREATED,
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn apply_deployment_approval(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    principal_id: Uuid,
    deployment_id: Uuid,
    expected_plan_hash: &str,
    decision: &str,
    reason: &str,
) -> Result<serde_json::Value, AppError> {
    if !matches!(decision, "approve" | "reject") {
        return Err(AppError::Validation("invalid decision".to_string()));
    }
    if expected_plan_hash.len() != 64 {
        return Err(AppError::Validation(
            "expected_plan_hash must be 64 hex chars".to_string(),
        ));
    }
    let row = sqlx::query_as::<_, (String, Option<String>, Option<Uuid>, Uuid, Uuid, Uuid)>(
        "SELECT d.state, d.plan_hash, d.requested_by, d.org_id, d.release_id, d.attempt_id
         FROM deployments d
         WHERE d.id = $1 AND d.repo_id = $2",
    )
    .bind(deployment_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((dep_state, stored_hash, requested_by, dep_org, release_id, attempt_id)) = row else {
        return Err(AppError::NotFound);
    };
    if dep_org != org_id {
        return Err(AppError::Forbidden);
    }
    if dep_state != "awaiting_approval" {
        return Err(AppError::Conflict(
            "deployment is not awaiting approval".to_string(),
        ));
    }
    let plan_hash = stored_hash.unwrap_or_default();
    if plan_hash != expected_plan_hash {
        return Err(AppError::Validation(
            "expected_plan_hash mismatch".to_string(),
        ));
    }
    let approver_ok = crate::grants::is_org_privileged(&state.pool, org_id, principal_id).await?
        || crate::grants::principal_has_repo_grant(&state.pool, repo_id, principal_id).await?;
    if !approver_ok {
        return Err(AppError::Forbidden);
    }
    if decision == "approve" && requested_by == Some(principal_id) {
        return Err(AppError::Conflict(
            "requester cannot approve own deployment".to_string(),
        ));
    }
    sqlx::query(
        "INSERT INTO deployment_approvals (id, org_id, repo_id, deployment_id, plan_hash, decision, approver_id, reason)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(deployment_id)
    .bind(&plan_hash)
    .bind(decision)
    .bind(principal_id)
    .bind(reason)
    .execute(&state.pool)
    .await?;
    let audit_action = if decision == "approve" {
        "deployment.approve"
    } else {
        "deployment.reject"
    };
    let _ = crate::audit::record(
        &state.pool,
        Some(org_id),
        Some(repo_id),
        principal_id,
        None,
        audit_action,
        "deployment",
        deployment_id,
        None,
        "allow",
        serde_json::json!({
            "decision": decision,
            "plan_hash": plan_hash,
            "reason": reason,
        }),
    )
    .await;
    if decision == "reject" {
        sqlx::query(
            "UPDATE deployments SET state = 'rejected', approved_by = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND state = 'awaiting_approval'",
        )
        .bind(deployment_id)
        .bind(principal_id)
        .execute(&state.pool)
        .await?;
        return Ok(serde_json::json!({
            "deployment_id": deployment_id.to_string(),
            "decision": decision,
            "state": "rejected",
            "plan_hash": plan_hash,
        }));
    }
    let updated = sqlx::query(
        "UPDATE deployments SET approved_by = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND state = 'awaiting_approval'",
    )
    .bind(deployment_id)
    .bind(principal_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict("deployment approval raced".to_string()));
    }
    resume_approved_deployment(state, deployment_id).await?;
    let new_state: (String,) = sqlx::query_as("SELECT state FROM deployments WHERE id = $1")
        .bind(deployment_id)
        .fetch_one(&state.pool)
        .await?;
    Ok(serde_json::json!({
        "deployment_id": deployment_id.to_string(),
        "decision": decision,
        "state": new_state.0,
        "plan_hash": plan_hash,
        "release_id": release_id.to_string(),
        "attempt_id": attempt_id.to_string(),
    }))
}

async fn resume_approved_deployment(
    state: &Arc<crate::AppState>,
    deployment_id: Uuid,
) -> Result<(), AppError> {
    let remote = runner_api::deploy_executor_remote();
    if remote {
        sqlx::query(
            "UPDATE deployments SET state = 'queued', updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND state = 'awaiting_approval'",
        )
        .bind(deployment_id)
        .execute(&state.pool)
        .await?;
        return Ok(());
    }
    let row = sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, Option<serde_json::Value>, String, Option<Uuid>)>(
        "SELECT d.org_id, d.repo_id, d.release_id, d.attempt_id, d.launch_payload, d.adapter_kind, d.requested_by
         FROM deployments d WHERE d.id = $1",
    )
    .bind(deployment_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((org_id, repo_id, release_id, attempt_id, payload_json, adapter_kind, requested_by)) =
        row
    else {
        return Err(AppError::NotFound);
    };
    let payload: DeploymentLaunchPayload = serde_json::from_value(
        payload_json.ok_or_else(|| AppError::Other("missing launch_payload".into()))?,
    )
    .map_err(|e| AppError::Other(e.to_string()))?;
    let release = sqlx::query_as::<_, (Option<String>, Option<String>, String)>(
        "SELECT manifest_sha256, oci_image_ref, commit_oid FROM releases WHERE id = $1",
    )
    .bind(release_id)
    .fetch_one(&state.pool)
    .await?;
    let (manifest, oci_image_ref, release_commit_oid) = release;
    let (compose_bin, resolved_adapter) = resolve_compose_adapter(
        &state.compose_adapter_bin,
        &state.compose_runner_bin,
        state.require_pipeline_oci,
    )
    .map_err(AppError::Validation)?;
    if resolved_adapter != adapter_kind {
        return Err(AppError::Conflict("adapter_kind drift".into()));
    }
    sqlx::query(
        "UPDATE deployments SET state = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND state = 'awaiting_approval'",
    )
    .bind(deployment_id)
    .execute(&state.pool)
    .await?;
    let env_slug = payload.environment_slug.clone();
    let launch = DeploymentLaunch {
        release_id,
        environment_slug: env_slug.clone(),
        health_target: Some(payload.health_target.clone()),
        attempt_id,
        rollback_of_id: payload.rollback_of_id,
        rollback_reason: payload.rollback_reason.clone(),
        audit_action: payload.audit_action.clone(),
    };
    let actor_id = payload
        .actor_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .or(requested_by);
    let _ = run_local_compose_deploy(
        state,
        deployment_id,
        org_id,
        repo_id,
        &launch,
        &env_slug,
        attempt_id,
        actor_id,
        None,
        &payload,
        &compose_bin,
        &adapter_kind,
        &payload.health_target,
        payload.publish_port,
        &manifest,
        &oci_image_ref,
        &release_commit_oid,
        payload.product_runner,
        &payload.teardown_project,
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct RollbackDeploymentRequest {
    pub target_deployment_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub reason: Option<String>,
}

/// Re-deploy the target deployment's **published release** through the compose adapter (same path as start).
pub async fn rollback_deployment(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Json(body): Json<RollbackDeploymentRequest>,
) -> Result<(StatusCode, Json<DeploymentView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let attempt_id = body.attempt_id.unwrap_or_else(Uuid::new_v4);
    let target_env = sqlx::query_as::<_, (Uuid,)>(
        "SELECT environment_id FROM deployments WHERE id = $1 AND repo_id = $2",
    )
    .bind(body.target_deployment_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((environment_id,)) = target_env else {
        return Err(AppError::NotFound);
    };
    mcp_rollback_deployment(
        &state,
        org_id,
        repo_id,
        environment_id,
        body.target_deployment_id,
        attempt_id,
        body.reason.clone(),
        Some(principal_id),
        crate::audit::request_id_from_headers(&headers),
    )
    .await
}

async fn mark_deployment_worker_failed(
    pool: &PgPool,
    deployment_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    error_code: &str,
    detail: &str,
) {
    let summary = serde_json::json!({
        "status": "unhealthy",
        "error_code": error_code,
        "detail": detail,
    });
    let _ = sqlx::query(
        "UPDATE deployments SET state = 'failed', health_summary = $4, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3 AND state = 'running'",
    )
    .bind(deployment_id)
    .bind(instance_id)
    .bind(epoch)
    .bind(summary)
    .execute(pool)
    .await;
}

/// Worker-side compose deploy (used when `MUSEHUB_DEPLOY_EXECUTOR=remote`).
#[allow(clippy::too_many_arguments)]
pub async fn execute_deployment_job(
    pool: &PgPool,
    deployment_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    compose_mock_bin: PathBuf,
    compose_runner_bin: PathBuf,
    require_pipeline_oci: bool,
    release_id: Uuid,
) -> Result<(), AppError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, Option<serde_json::Value>)>(
        "SELECT org_id, repo_id, state, launch_payload FROM deployments WHERE id = $1",
    )
    .bind(deployment_id)
    .fetch_optional(pool)
    .await?;
    let Some((org_id, repo_id, state, payload_json)) = row else {
        return Err(AppError::NotFound);
    };
    if state != "running" {
        return Ok(());
    }
    let holder = sqlx::query_as::<_, (Option<Uuid>, i64)>(
        "SELECT lease_holder, lease_epoch FROM deployments WHERE id = $1",
    )
    .bind(deployment_id)
    .fetch_one(pool)
    .await?;
    if holder.0 != Some(instance_id) || holder.1 != epoch {
        return Err(AppError::Conflict("deployment lease fenced".into()));
    }
    let payload: DeploymentLaunchPayload =
        match serde_json::from_value(payload_json.unwrap_or(serde_json::json!({}))) {
            Ok(p) => p,
            Err(e) => {
                mark_deployment_worker_failed(
                    pool,
                    deployment_id,
                    instance_id,
                    epoch,
                    "launch_payload_invalid",
                    &e.to_string(),
                )
                .await;
                return Err(AppError::Other(e.to_string()));
            }
        };
    let (compose_bin, adapter_kind) =
        match resolve_compose_adapter(&compose_mock_bin, &compose_runner_bin, require_pipeline_oci)
        {
            Ok(v) => v,
            Err(e) => {
                mark_deployment_worker_failed(
                    pool,
                    deployment_id,
                    instance_id,
                    epoch,
                    "adapter_resolution_failed",
                    &e,
                )
                .await;
                return Err(AppError::Validation(e));
            }
        };
    if adapter_kind != payload.adapter_kind.as_str() {
        mark_deployment_worker_failed(
            pool,
            deployment_id,
            instance_id,
            epoch,
            "adapter_kind_mismatch",
            "adapter kind mismatch",
        )
        .await;
        return Err(AppError::Validation("adapter kind mismatch".into()));
    }
    let mut cmd = Command::new(&compose_bin);
    cmd.env("MUSEHUB_DEPLOY_HEALTH_TARGET", &payload.health_target)
        .env(
            "MUSEHUB_RELEASE_MANIFEST_SHA256",
            payload.manifest.clone().unwrap_or_default(),
        )
        .env("MUSEHUB_COMPOSE_PROJECT", &payload.project)
        .env(
            "MUSEHUB_COMPOSE_PUBLISH_PORT",
            payload.publish_port.to_string(),
        )
        .env("MUSEHUB_RELEASE_COMMIT_OID", &payload.release_commit_oid)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(ref oci) = payload.oci_image_ref {
        cmd.env("MUSEHUB_OCI_IMAGE_REF", oci);
    }
    if let Some(ref prev) = payload.teardown_project {
        cmd.env("MUSEHUB_COMPOSE_TEARDOWN_PROJECT", prev);
    }
    if payload.product_runner
        || (payload.rollback_of_id.is_some() && adapter_kind == "compose-runner")
    {
        cmd.env("MUSEHUB_COMPOSE_LEAVE_STACK", "1");
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("spawn compose adapter: {e}");
            mark_deployment_worker_failed(
                pool,
                deployment_id,
                instance_id,
                epoch,
                "spawn_failed",
                &msg,
            )
            .await;
            return Err(AppError::Other(msg));
        }
    };
    let finished = tokio::time::timeout(Duration::from_secs(300), child.wait_with_output()).await;
    let (ok, adapter_stdout, capability_gap) = match &finished {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let cap = out.status.code() == Some(2);
            (out.status.success(), stdout, cap)
        }
        _ => (false, String::new(), true),
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
        match (payload.oci_image_ref.as_ref(), applied_digest.as_ref()) {
            (Some(req), Some(app)) => req.split('@').nth(1) == Some(app.as_str()),
            (Some(_), None) => false,
            _ => false,
        }
    } else {
        true
    };
    let source_ok = if adapter_kind == "compose-runner" {
        verified_source_oid.as_deref() == Some(payload.release_commit_oid.as_str())
    } else {
        true
    };
    let deploy_ok = ok && digest_ok && source_ok;
    let mut summary = if deploy_ok {
        serde_json::json!({
            "status": "healthy",
            "target": payload.health_target,
            "adapter": adapter_kind,
            "release_manifest_sha256": payload.manifest,
            "release_oci_image_ref": payload.oci_image_ref,
            "applied_oci_digest": applied_digest,
            "verified_source_oid": verified_source_oid,
            "pipeline_source_oid": payload.release_commit_oid,
            "compose_project": payload.project,
        })
    } else if capability_gap {
        serde_json::json!({
            "status": "unhealthy",
            "adapter": adapter_kind,
            "error_code": "docker_unavailable",
            "compose_project": payload.project,
        })
    } else {
        serde_json::json!({
            "status": "unhealthy",
            "adapter": adapter_kind,
            "error_code": "deploy_failed",
            "compose_project": payload.project,
        })
    };
    if let Some(obj) = summary.as_object_mut() {
        if let Some(target_id) = payload.rollback_of_id {
            obj.insert("rollback".into(), serde_json::json!(true));
            obj.insert(
                "restored_from".into(),
                serde_json::json!(target_id.to_string()),
            );
        }
    }
    let state_name = if deploy_ok { "succeeded" } else { "failed" };
    sqlx::query(
        "UPDATE deployments SET state = $2, health_summary = $3, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND lease_holder = $4 AND lease_epoch = $5",
    )
    .bind(deployment_id)
    .bind(state_name)
    .bind(summary)
    .bind(instance_id)
    .bind(epoch)
    .execute(pool)
    .await?;
    let updated = sqlx::query_as::<_, (String,)>("SELECT state FROM deployments WHERE id = $1")
        .bind(deployment_id)
        .fetch_one(pool)
        .await?;
    if updated.0 == "running" {
        mark_deployment_worker_failed(
            pool,
            deployment_id,
            instance_id,
            epoch,
            "state_update_missed",
            "deployment terminal update lost lease match",
        )
        .await;
        return Err(AppError::Conflict("deployment state not updated".into()));
    }
    if let Some(actor_str) = payload.actor_id.as_ref() {
        if let Ok(actor) = Uuid::parse_str(actor_str) {
            let _ = crate::audit::record(
                pool,
                Some(org_id),
                Some(repo_id),
                actor,
                None,
                &payload.audit_action,
                "deployment",
                deployment_id,
                None,
                if deploy_ok { "allow" } else { "deny" },
                serde_json::json!({
                    "state": state_name,
                    "release_id": release_id.to_string(),
                }),
            )
            .await;
        }
    }
    Ok(())
}
