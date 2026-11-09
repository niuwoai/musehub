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
    audit_action: &'static str,
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
        audit_action: "deployment.start",
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
        audit_action: "deployment.start",
    };
    let pat = state.mcp_pat_slot.read().await;
    let actor = pat.as_ref().map(|p| p.principal_id);
    drop(pat);
    let (status, Json(view)) =
        launch_deployment(state, org_id, repo_id, actor, None, launch).await?;
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

async fn launch_deployment(
    state: &Arc<crate::AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    actor_id: Option<Uuid>,
    request_id: Option<Uuid>,
    launch: DeploymentLaunch,
) -> Result<(StatusCode, Json<DeploymentView>), AppError> {
    let env_slug = launch.environment_slug.clone();
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
    let insert = sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id, rollback_of_id)
         VALUES ($1,$2,$3,$4,$5,$6,'running',$7,$8)",
    )
    .bind(deployment_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(environment_id)
    .bind(launch.release_id)
    .bind(adapter_kind)
    .bind(attempt_id)
    .bind(launch.rollback_of_id)
    .execute(&state.pool)
    .await;

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
                    StatusCode::OK,
                ));
            }
        }
    }
    insert?;

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
    let project = if product_runner || launch.rollback_of_id.is_some() {
        format!("musehub-env-{environment_id}")
    } else {
        format!("musehub-deploy-{deployment_id}")
    };
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
    if let Some(ref prev) = teardown_project {
        cmd.env("MUSEHUB_COMPOSE_TEARDOWN_PROJECT", prev);
    }
    if product_runner || (launch.rollback_of_id.is_some() && adapter_kind == "compose-runner") {
        cmd.env("MUSEHUB_COMPOSE_LEAVE_STACK", "1");
    }
    let child = cmd
        .spawn()
        .map_err(|e| AppError::Other(format!("spawn {:?}: {e}", compose_bin)))?;
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
            launch.audit_action,
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
        &env_slug,
        state_name.to_string(),
        adapter_kind.to_string(),
        Some(summary),
        attempt_id,
        StatusCode::CREATED,
    ))
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
    let target = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Option<serde_json::Value>)>(
        "SELECT d.id, d.environment_id, d.release_id, d.state, d.health_summary
         FROM deployments d WHERE d.id = $1 AND d.repo_id = $2",
    )
    .bind(body.target_deployment_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((target_id, _environment_id, release_id, dep_state, health_summary)) = target else {
        return Err(AppError::NotFound);
    };
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
        // New runner publish port; reuse env compose project via launch_deployment.
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
        rollback_reason: body.reason.clone(),
        audit_action: "deployment.rollback",
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
