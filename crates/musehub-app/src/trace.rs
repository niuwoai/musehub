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

#[derive(Serialize)]
pub struct IssueDeliveryLinkPr {
    pub id: Uuid,
    pub number: i64,
    pub state: String,
    pub merged_oid: Option<String>,
}

#[derive(Serialize)]
pub struct IssueDeliveryLinkPipeline {
    pub id: Uuid,
    pub state: String,
    pub source_oid: String,
}

#[derive(Serialize)]
pub struct IssueDeliveryLinkDeployment {
    pub id: Uuid,
    pub environment_slug: String,
    pub state: String,
}

#[derive(Serialize)]
pub struct IssueDeliveryTrace {
    pub issue_id: Uuid,
    pub issue_number: i64,
    pub pull_requests: Vec<IssueDeliveryLinkPr>,
    pub pipeline_runs: Vec<IssueDeliveryLinkPipeline>,
    pub deployments: Vec<IssueDeliveryLinkDeployment>,
    pub gaps: Vec<String>,
}

pub async fn get_issue_delivery_trace(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, issue_number)): AxumPath<(String, String, i64)>,
) -> Result<Json<IssueDeliveryTrace>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let issue =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE repo_id = $1 AND number = $2")
            .bind(repo_id)
            .bind(issue_number)
            .fetch_optional(&state.pool)
            .await?;
    let Some((issue_id,)) = issue else {
        return Err(AppError::NotFound);
    };
    let mut gaps = Vec::new();
    let prs = sqlx::query_as::<_, (Uuid, i64, String, Option<String>)>(
        "SELECT id, number, state, merged_oid FROM pull_requests
         WHERE repo_id = $1 AND issue_id = $2 ORDER BY number DESC LIMIT 20",
    )
    .bind(repo_id)
    .bind(issue_id)
    .fetch_all(&state.pool)
    .await?;
    if prs.is_empty() {
        gaps.push("no_pull_requests".to_string());
    }
    let pipelines = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, state, source_oid FROM pipeline_runs
         WHERE repo_id = $1 AND issue_id = $2 ORDER BY created_at DESC LIMIT 20",
    )
    .bind(repo_id)
    .bind(issue_id)
    .fetch_all(&state.pool)
    .await?;
    if pipelines.is_empty() {
        gaps.push("no_pipeline_runs".to_string());
    }
    let deployments = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT d.id, e.slug, d.state FROM deployments d
         JOIN environments e ON e.id = d.environment_id
         WHERE d.repo_id = $1 AND d.id IN (
           SELECT dep.id FROM deployments dep
           JOIN releases r ON r.id = dep.release_id
           JOIN pipeline_runs pr ON pr.id = r.pipeline_run_id
           WHERE pr.issue_id = $2
         )
         ORDER BY d.created_at DESC LIMIT 20",
    )
    .bind(repo_id)
    .bind(issue_id)
    .fetch_all(&state.pool)
    .await?;
    if deployments.is_empty() {
        gaps.push("no_deployments".to_string());
    }
    Ok(Json(IssueDeliveryTrace {
        issue_id,
        issue_number,
        pull_requests: prs
            .into_iter()
            .map(|(id, number, state, merged_oid)| IssueDeliveryLinkPr {
                id,
                number,
                state,
                merged_oid,
            })
            .collect(),
        pipeline_runs: pipelines
            .into_iter()
            .map(|(id, state, source_oid)| IssueDeliveryLinkPipeline {
                id,
                state,
                source_oid,
            })
            .collect(),
        deployments: deployments
            .into_iter()
            .map(
                |(id, environment_slug, state)| IssueDeliveryLinkDeployment {
                    id,
                    environment_slug,
                    state,
                },
            )
            .collect(),
        gaps,
    }))
}
