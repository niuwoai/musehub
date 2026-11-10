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
    pub pr_number: Option<i64>,
    pub merged_oid: Option<String>,
    pub issue_id: Option<Uuid>,
    pub issue_number: Option<i64>,
    pub issue_revision_id: Option<Uuid>,
    pub plan_accepted: Option<bool>,
    pub agent_run_id: Option<Uuid>,
    pub deployment_adapter: String,
    pub adapter_is_mock: bool,
    pub release_oci_image_ref: Option<String>,
    pub applied_oci_digest: Option<String>,
    pub verified_source_oid: Option<String>,
    pub artifacts: Vec<TraceArtifact>,
    pub gaps: Vec<String>,
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
    let release = sqlx::query_as::<_, (String, Uuid, Option<String>)>(
        "SELECT commit_oid, pipeline_run_id, oci_image_ref FROM releases WHERE id = $1",
    )
    .bind(release_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| AppError::NotFound)?;
    let (commit_oid, pipeline_run_id, release_oci) = release;
    let dep = sqlx::query_as::<_, (String, Option<serde_json::Value>)>(
        "SELECT adapter_kind, health_summary FROM deployments WHERE id = $1",
    )
    .bind(deployment_id)
    .fetch_one(&state.pool)
    .await?;
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
    let pr_id = pipe.0.or(pr_from_link);
    let mut gaps = Vec::new();
    let (pr_number, merged_oid) = if let Some(pid) = pr_id {
        let row = sqlx::query_as::<_, (i64, Option<String>)>(
            "SELECT number, merged_oid FROM pull_requests WHERE id = $1",
        )
        .bind(pid)
        .fetch_optional(&state.pool)
        .await?;
        match row {
            Some((n, m)) => (Some(n), m),
            None => {
                gaps.push("pr_record_missing".to_string());
                (None, None)
            }
        }
    } else {
        gaps.push("pr_not_linked".to_string());
        (None, None)
    };
    let issue_id = pipe.1;
    let (issue_number, issue_revision_id, plan_accepted) = match issue_id {
        None => {
            gaps.push("issue_not_linked".to_string());
            (None, None, None)
        }
        Some(iid) => match sqlx::query_as::<_, (i64, Option<Uuid>)>(
            "SELECT number, current_revision_id FROM issues WHERE id = $1",
        )
        .bind(iid)
        .fetch_optional(&state.pool)
        .await?
        {
            None => {
                gaps.push("issue_record_missing".to_string());
                (None, None, None)
            }
            Some((num, rev)) => {
                if let Some(rev_id) = rev {
                    let accepted: (i64,) = sqlx::query_as(
                        "SELECT COUNT(*) FROM implementation_plans
                         WHERE issue_revision_id = $1 AND state = 'accepted'",
                    )
                    .bind(rev_id)
                    .fetch_one(&state.pool)
                    .await?;
                    (Some(num), Some(rev_id), Some(accepted.0 > 0))
                } else {
                    gaps.push("issue_revision_missing".to_string());
                    (Some(num), None, None)
                }
            }
        },
    };
    if pipe.2.is_none() {
        gaps.push("agent_run_not_linked".to_string());
    }
    let artifacts = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT name, sha256, byte_length FROM artifacts
         WHERE pipeline_run_id = $1 AND state = 'ready' ORDER BY name",
    )
    .bind(pipeline_run_id)
    .fetch_all(&state.pool)
    .await?;
    if artifacts.is_empty() {
        gaps.push("no_ready_artifacts".to_string());
    }
    let adapter = dep.0.clone();
    let applied_oci_digest = dep.1.as_ref().and_then(|s| {
        s.get("applied_oci_digest")
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    let verified_source_oid = dep.1.as_ref().and_then(|s| {
        s.get("verified_source_oid")
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    if adapter == "compose-mock" {
        gaps.push("deploy_not_isolated_compose".to_string());
        gaps.push("deploy_no_oci_registry".to_string());
    } else if adapter == "compose-runner" {
        if release_oci.is_none() {
            gaps.push("release_oci_image_ref_missing".to_string());
        }
        if applied_oci_digest.is_none() {
            gaps.push("applied_oci_digest_missing".to_string());
        }
        if verified_source_oid.as_deref() != Some(commit_oid.as_str()) {
            gaps.push("verified_source_oid_mismatch".to_string());
        }
        if let Some(summary) = &dep.1 {
            if summary.get("error_code").and_then(|v| v.as_str()) == Some("docker_unavailable") {
                gaps.push("deploy_docker_unavailable".to_string());
            }
        }
    }
    Ok(Json(TraceView {
        deployment_id,
        release_id,
        pipeline_run_id,
        commit_oid,
        pr_id,
        pr_number,
        merged_oid,
        issue_id,
        issue_number,
        issue_revision_id,
        plan_accepted,
        agent_run_id: pipe.2,
        deployment_adapter: adapter.clone(),
        adapter_is_mock: adapter == "compose-mock",
        release_oci_image_ref: release_oci,
        applied_oci_digest,
        verified_source_oid,
        artifacts: artifacts
            .into_iter()
            .map(|(name, sha256, byte_length)| TraceArtifact {
                name,
                sha256,
                byte_length,
            })
            .collect(),
        gaps,
    }))
}
