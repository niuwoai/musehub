//! Product HTTP API: auth, repositories, Git smart HTTP, issues, reference agent runs.

#![allow(clippy::type_complexity)]

pub mod agent_supervisor;
pub mod pipeline_supervisor;
pub use git_http::{bare_repo_path, init_bare_repository};
mod artifacts;
mod auth;
mod bootstrap;
mod codex;
mod compose;
mod deployment;
mod error;
mod git_http;
mod hooks;
pub mod lease;
pub mod mcp;
mod members;
pub mod pat;
mod pat_auth;
mod pat_policy;
mod pipeline;
mod recovery;
pub use pipeline::seed_repo_environment;
pub use recovery::{reconcile_orphaned_work, ReconcileReport};
mod pr;
mod release;
mod security;
mod trace;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use base64::Engine;
use musehub_agent_adapter::REFERENCE_ADAPTER;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::fs;
use uuid::Uuid;

use crate::agent_supervisor::AgentSupervisor;
use crate::pipeline_supervisor::PipelineSupervisor;

use crate::auth::{
    clear_session_cookie, cookie_name, login, logout, principal_from_session, session_cookie,
    verify_password,
};
use crate::bootstrap::seed_if_empty;
use crate::error::AppError;
use crate::git_http::{git_clone_url, git_http_backend};

/// Application state for product routes.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub git_root: PathBuf,
    pub artifact_root: PathBuf,
    pub bind_display: String,
    pub reference_adapter_bin: PathBuf,
    pub pipeline_adapter_bin: PathBuf,
    pub compose_adapter_bin: PathBuf,
    pub compose_runner_bin: PathBuf,
    /// Control-plane instance identity for worker leases.
    pub instance_id: Uuid,
    pub supervisor: Arc<AgentSupervisor>,
    pub pipeline_supervisor: Arc<PipelineSupervisor>,
    /// When true: release publish requires pipeline OCI artifact; deploy rejects mock health / mock compose.
    pub require_pipeline_oci: bool,
    /// Request-scoped PAT for `/mcp` tools (set by MCP Bearer middleware).
    pub mcp_pat_slot: Arc<tokio::sync::RwLock<Option<pat_auth::PatContext>>>,
    /// Streamable HTTP `mcp-session-id` → authenticated PAT (session follow-up POST/GET).
    pub mcp_session_pats: Arc<tokio::sync::RwLock<std::collections::HashMap<String, pat_auth::PatContext>>>,
}

pub fn require_pipeline_oci_from_env() -> bool {
    std::env::var("MUSEHUB_REQUIRE_PIPELINE_OCI")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn product_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/api/v1/auth/login", post(login_handler))
        .route("/api/v1/auth/logout", post(logout_handler))
        .route("/api/v1/me", get(me_handler))
        .route("/api/v1/tokens", post(create_pat_handler))
        .route("/api/v1/tokens/{token_id}", delete(revoke_pat_handler))
        .route("/api/v1/orgs/{org}/members", post(members::invite_member))
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/collaborators",
            post(members::grant_repo_collaborator),
        )
        .route(
            "/api/v1/orgs/{org}/repos",
            get(list_repos).post(create_repo),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/issues",
            get(list_issues).post(create_issue),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/issues/{issue_number}/plans",
            post(create_plan).patch(accept_plan),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/agent-runs",
            post(start_agent_run),
        )
        .route("/api/v1/agent-runs/{run_id}", get(get_agent_run))
        .route("/api/v1/agent-runs/{run_id}/cancel", post(cancel_agent_run))
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pull-requests",
            get(pr::list_pull_requests).post(pr::create_pull_request),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pull-requests/{number}/reviews",
            post(pr::review_pull_request),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pull-requests/{number}/merge",
            post(pr::merge_pull_request),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pull-requests/{number}/sync",
            patch(pr::sync_pull_request),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pull-requests/{number}/checks/{check_name}",
            patch(pr::set_pull_request_check),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pull-requests/{number}/validation-runs",
            post(pipeline::start_pr_validation_run),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pipeline-runs",
            post(pipeline::start_pipeline_run),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pipeline-runs/{run_id}",
            get(pipeline::get_pipeline_run),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/pipeline-runs/{run_id}/cancel",
            post(pipeline::cancel_pipeline_run),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/releases",
            post(release::create_release),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/releases/{release_id}/publish",
            post(release::publish_release),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/deployments",
            post(deployment::start_deployment),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/trace/deployments/{deployment_id}",
            get(trace::get_deployment_trace),
        )
        .route(
            "/api/v1/orgs/{org}/repos/{repo}/runner/check-reports",
            post(pr::runner_report_check),
        )
        .route(
            "/api/v1/adapters/codex/probe",
            post(codex::codex_probe_handler),
        )
        .route(
            "/api/v1/adapters/codex/status",
            get(codex::codex_status_handler),
        )
        .route(
            "/api/v1/adapters/codex/task",
            post(codex::codex_task_handler),
        )
        .route(
            "/git/{org}/{repo_git}/{*git_path}",
            axum::routing::any(git_smart_http),
        )
        .with_state(state.clone());
    mcp::merge_product_mcp(api, state)
}

/// Integration-test helper for seeding credentials (not for production callers).
pub fn hash_password_for_tests(password: &str) -> Result<String, AppError> {
    auth::hash_password(password)
}

pub async fn prepare(
    pool: PgPool,
    git_root: PathBuf,
    bootstrap_password: Option<&str>,
) -> Result<(), AppError> {
    if let Some(password) = bootstrap_password {
        seed_if_empty(&pool, password).await?;
    }
    fs::create_dir_all(&git_root)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    Ok(())
}

/// Run once at process boot (not on every `prepare` call).
pub async fn boot_reconcile(pool: &PgPool) -> Result<ReconcileReport, AppError> {
    reconcile_orphaned_work(pool).await
}

#[derive(Deserialize)]
struct LoginRequest {
    handle: String,
    password: String,
}

#[derive(Serialize)]
struct PrincipalView {
    id: Uuid,
    handle: String,
}

async fn login_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let (_id, token) = login(&state.pool, &body.handle, &body.password).await?;
    let mut response = Json(serde_json::json!({ "ok": true })).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        session_cookie(&token).to_string().parse().unwrap(),
    );
    Ok(response)
}

async fn logout_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(token) = session_token_from_headers(&headers) {
        logout(&state.pool, &token).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        clear_session_cookie().to_string().parse().unwrap(),
    );
    Ok(response)
}

async fn me_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<PrincipalView>, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let row = sqlx::query_as::<_, (String,)>("SELECT handle FROM principals WHERE id = $1")
        .bind(principal_id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(PrincipalView {
        id: principal_id,
        handle: row.0,
    }))
}

#[derive(Serialize)]
struct RepositoryView {
    id: Uuid,
    slug: String,
    state: String,
    clone_url: String,
}

async fn list_repos(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Json<Vec<RepositoryView>>, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let org_id = org_id_for_member(&state.pool, &org, principal_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT r.id, r.slug, r.state
         FROM repositories r
         WHERE r.org_id = $1
         ORDER BY r.created_at",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, slug, repo_state)| RepositoryView {
                id,
                slug: slug.clone(),
                state: repo_state,
                clone_url: git_clone_url(&state.bind_display, &org, &slug),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct CreateRepoRequest {
    slug: String,
    description: Option<String>,
}

async fn create_repo(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<CreateRepoRequest>,
) -> Result<(StatusCode, Json<RepositoryView>), AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let org_id = org_id_for_member(&state.pool, &org, principal_id).await?;
    let slug = normalize_slug(&body.slug)?;
    let storage_key = Uuid::new_v4();
    let repo_id = Uuid::new_v4();
    git_http::init_bare_repository(&state.git_root, &storage_key)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, description, storage_key, state)
         VALUES ($1, $2, $3, $4, $5, $6, 'ready')",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(&slug)
    .bind(slug.to_lowercase())
    .bind(body.description.unwrap_or_default())
    .bind(storage_key)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1, $2, $3, $4, 'maintainer')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    pr::seed_repo_required_checks(&state.pool, repo_id).await?;
    pipeline::seed_repo_environment(&state.pool, org_id, repo_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(RepositoryView {
            id: repo_id,
            slug: slug.clone(),
            state: "ready".to_string(),
            clone_url: git_clone_url(&state.bind_display, &org, &slug),
        }),
    ))
}

#[derive(Serialize)]
struct IssueView {
    id: Uuid,
    number: i64,
    title: String,
    state: String,
}

async fn list_issues(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
) -> Result<Json<Vec<IssueView>>, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let repo_id = repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, i64, String, String)>(
        "SELECT id, number, title, state FROM issues WHERE repo_id = $1 ORDER BY number",
    )
    .bind(repo_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, number, title, issue_state)| IssueView {
                id,
                number,
                title,
                state: issue_state,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct CreateIssueRequest {
    title: String,
    body: Option<String>,
}

async fn create_issue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Json(body): Json<CreateIssueRequest>,
) -> Result<(StatusCode, Json<IssueView>), AppError> {
    if body.title.trim().is_empty() {
        return Err(AppError::Validation("title required".to_string()));
    }
    let principal_id = require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) = repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let mut tx = state.pool.begin().await?;
    let number: (i64,) = sqlx::query_as(
        "UPDATE repositories SET next_issue_number = next_issue_number + 1
         WHERE id = $1 RETURNING next_issue_number - 1",
    )
    .bind(repo_id)
    .fetch_one(&mut *tx)
    .await?;
    let title = body.title.trim().to_string();
    let issue_body = body.body.unwrap_or_default();
    let issue_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, author_id, title, body)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(issue_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(number.0)
    .bind(principal_id)
    .bind(&title)
    .bind(&issue_body)
    .execute(&mut *tx)
    .await?;
    let revision_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issue_revisions (id, org_id, repo_id, issue_id, revision, title, body, author_id)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7)",
    )
    .bind(revision_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(issue_id)
    .bind(&title)
    .bind(&issue_body)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE issues SET current_revision_id = $2 WHERE id = $1")
        .bind(issue_id)
        .bind(revision_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(IssueView {
            id: issue_id,
            number: number.0,
            title,
            state: "open".to_string(),
        }),
    ))
}

#[derive(Deserialize)]
struct StartAgentRunRequest {
    issue_id: Uuid,
    message: String,
    budget_tokens: Option<u64>,
}

#[derive(Serialize)]
struct AgentRunView {
    id: Uuid,
    adapter_kind: String,
    state: String,
    result_commit_oid: Option<String>,
    usage_tokens: u64,
    budget_unknown: bool,
    error_code: Option<String>,
}

async fn start_agent_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Json(body): Json<StartAgentRunRequest>,
) -> Result<Json<AgentRunView>, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let issue =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE id = $1 AND repo_id = $2")
            .bind(body.issue_id)
            .bind(repo_id)
            .fetch_optional(&state.pool)
            .await?;
    if issue.is_none() {
        return Err(AppError::NotFound);
    }
    let plan_ok = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM implementation_plans p
         JOIN issues i ON i.id = $1 AND i.repo_id = $2
         WHERE p.state = 'accepted' AND p.issue_revision_id = i.current_revision_id",
    )
    .bind(body.issue_id)
    .bind(repo_id)
    .fetch_one(&state.pool)
    .await?;
    if plan_ok.0 == 0 {
        return Err(AppError::Validation(
            "agent run requires an accepted plan bound to the issue's current revision".to_string(),
        ));
    }
    let run_id = Uuid::new_v4();
    let branch = format!("agent/{run_id}");
    let default_branch: (String,) =
        sqlx::query_as("SELECT default_branch FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_one(&state.pool)
            .await?;
    security::validate_agent_work_branch(&branch, &default_branch.0)
        .map_err(AppError::Validation)?;
    let instance_id = state.instance_id;
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, requested_by, adapter_kind, work_branch, state, budget_tokens_limit, lease_holder, lease_until)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued', $8, $9,
           CURRENT_TIMESTAMP + ($10::text || ' seconds')::interval)",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(body.issue_id)
    .bind(principal_id)
    .bind(REFERENCE_ADAPTER)
    .bind(&branch)
    .bind(body.budget_tokens.map(|limit| limit as i64))
    .bind(instance_id)
    .bind(crate::lease::LEASE_SECS)
    .execute(&state.pool)
    .await?;
    let supervisor = state.supervisor.clone();
    let pool = state.pool.clone();
    let git_root = state.git_root.clone();
    let reference_bin = state.reference_adapter_bin.clone();
    let message = body.message.clone();
    let deadline = std::time::Duration::from_secs(120);
    let budget_tokens = body.budget_tokens;
    tokio::spawn(async move {
        agent_supervisor::run_reference_job(
            pool,
            supervisor,
            instance_id,
            run_id,
            repo_id,
            storage_key,
            git_root,
            reference_bin,
            branch,
            message,
            deadline,
            budget_tokens,
        )
        .await;
    });
    Ok(Json(AgentRunView {
        id: run_id,
        adapter_kind: REFERENCE_ADAPTER.to_string(),
        state: "queued".to_string(),
        result_commit_oid: None,
        usage_tokens: 0,
        budget_unknown: true,
        error_code: None,
    }))
}

async fn get_agent_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Result<Json<AgentRunView>, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let row = sqlx::query_as::<_, (Uuid, String, String, Option<String>, i64, Option<String>)>(
        "SELECT id, adapter_kind, state, result_commit_oid, usage_tokens, error_code
         FROM agent_runs WHERE id = $1 AND requested_by = $2",
    )
    .bind(run_id)
    .bind(principal_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, adapter_kind, run_state, commit, tokens, error_code)) = row else {
        return Err(AppError::NotFound);
    };
    Ok(Json(AgentRunView {
        id,
        adapter_kind,
        state: run_state,
        result_commit_oid: commit,
        usage_tokens: tokens as u64,
        budget_unknown: true,
        error_code,
    }))
}

async fn cancel_agent_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let updated = sqlx::query(
        "UPDATE agent_runs SET state = 'cancel_requested', updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND requested_by = $2 AND state IN ('queued', 'running')",
    )
    .bind(run_id)
    .bind(principal_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    let _ = state.supervisor.cancel(run_id).await;
    Ok(StatusCode::ACCEPTED)
}

async fn git_smart_http(
    State(state): State<Arc<AppState>>,
    method: Method,
    Path((org, repo_git, git_path)): Path<(String, String, String)>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let (user, pass) = basic_auth(&headers)?;
    let org_id = org_id_from_slug(&state.pool, &org).await?;
    let principal_id = if pass.starts_with("mhp_") {
        let pat_ctx = pat_auth::authenticate_pat_context(&state.pool, &pass).await?;
        if !pat_auth::pat_org_allowed(pat_ctx.org_id, org_id) {
            return Err(AppError::Forbidden);
        }
        let service = query
            .iter()
            .find(|(key, _)| key == "service")
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        let needs_write = service.contains("receive-pack") || method == Method::POST;
        let needs_read = needs_write || service.contains("upload-pack") || method == Method::GET;
        if needs_write && !pat_auth::scope_allows(&pat_ctx.scopes, "repo:write") {
            return Err(AppError::Forbidden);
        }
        if needs_read && !pat_auth::scope_allows(&pat_ctx.scopes, "repo:read") {
            return Err(AppError::Forbidden);
        }
        pat_ctx.principal_id
    } else {
        authenticate_basic(&state.pool, &user, &pass).await?
    };
    let repo = repo_git.strip_suffix(".git").ok_or(AppError::Validation(
        "repository URL must end with .git".to_string(),
    ))?;
    let storage_key = repo_storage_for_member(&state.pool, &org, repo, principal_id)
        .await?
        .2;
    let remote_user = user;
    let query_string = serde_urlencoded::to_string(&query).unwrap_or_default();
    security::validate_git_suffix(&git_path).map_err(AppError::Validation)?;
    let suffix = if git_path.is_empty() {
        String::new()
    } else {
        format!("/{git_path}")
    };
    git_http_backend(
        &method,
        &storage_key,
        &suffix,
        &query_string,
        &headers,
        body,
        &state.git_root,
        &remote_user,
    )
    .await
}

pub(crate) async fn require_session(pool: &PgPool, headers: &HeaderMap) -> Result<Uuid, AppError> {
    let token = session_token_from_headers(headers).ok_or(AppError::Unauthorized)?;
    principal_from_session(pool, &token).await
}

fn session_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{}=", cookie_name())) {
            return Some(value.to_string());
        }
    }
    None
}

fn basic_auth(headers: &HeaderMap) -> Result<(String, String), AppError> {
    let header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let encoded = header
        .strip_prefix("Basic ")
        .ok_or(AppError::Unauthorized)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| AppError::Unauthorized)?;
    let text = String::from_utf8(decoded).map_err(|_| AppError::Unauthorized)?;
    let (user, pass) = text.split_once(':').ok_or(AppError::Unauthorized)?;
    Ok((user.to_string(), pass.to_string()))
}

async fn revoke_pat_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let updated = sqlx::query(
        "UPDATE access_tokens SET revoked_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND principal_id = $2 AND revoked_at IS NULL",
    )
    .bind(token_id)
    .bind(principal_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn create_pat_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreatePatRequest>,
) -> Result<Json<PatCreatedView>, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let org_id = if let Some(org) = &body.org {
        Some(org_id_for_member(&state.pool, org, principal_id).await?)
    } else {
        None
    };
    let (id, token) = if let Some(scopes) = &body.scopes {
        pat_policy::validate_scope_issuance(scopes)?;
        let token_owner = if scopes.iter().any(|s| s == "checks:write") {
            let org = org_id.ok_or_else(|| {
                AppError::Validation("checks:write tokens require org binding".to_string())
            })?;
            pat_policy::require_org_admin_to_issue_runner_token(
                &state.pool,
                org,
                principal_id,
                scopes,
            )
            .await?;
            let handle = body.runner_handle.as_deref().ok_or_else(|| {
                AppError::Validation(
                    "checks:write tokens require runner_handle for the delegated runner principal"
                        .to_string(),
                )
            })?;
            pat_policy::resolve_runner_token_principal(&state.pool, org, handle).await?
        } else {
            principal_id
        };
        let refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
        pat::create_pat_with_scopes(&state.pool, token_owner, org_id, &body.name, &refs).await?
    } else {
        pat::create_pat(&state.pool, principal_id, org_id, &body.name).await?
    };
    Ok(Json(PatCreatedView {
        id,
        token,
        note: "shown once",
    }))
}

#[derive(Deserialize)]
struct CreatePatRequest {
    name: String,
    org: Option<String>,
    scopes: Option<Vec<String>>,
    /// Required when issuing `checks:write`: non-admin member who will use the token.
    runner_handle: Option<String>,
}

#[derive(Serialize)]
struct PatCreatedView {
    id: Uuid,
    token: String,
    note: &'static str,
}

#[derive(Deserialize)]
struct CreatePlanRequest {
    body: String,
}

#[derive(Serialize)]
struct PlanView {
    id: Uuid,
    version: i32,
    state: String,
}

async fn create_plan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, issue_number)): Path<(String, String, i64)>,
    Json(body): Json<CreatePlanRequest>,
) -> Result<(StatusCode, Json<PlanView>), AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) = repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let revision = sqlx::query_as::<_, (Uuid,)>(
        "SELECT r.id FROM issue_revisions r
         JOIN issues i ON i.id = r.issue_id
         WHERE i.repo_id = $1 AND i.number = $2 AND r.revision = (
           SELECT MAX(revision) FROM issue_revisions WHERE issue_id = i.id
         )",
    )
    .bind(repo_id)
    .bind(issue_number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((revision_id,)) = revision else {
        return Err(AppError::NotFound);
    };
    let version: (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM implementation_plans WHERE issue_revision_id = $1",
    )
    .bind(revision_id)
    .fetch_one(&state.pool)
    .await?;
    let plan_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO implementation_plans (id, org_id, repo_id, issue_revision_id, version, body, state)
         VALUES ($1, $2, $3, $4, $5, $6, 'draft')",
    )
    .bind(plan_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(revision_id)
    .bind(version.0)
    .bind(body.body)
    .execute(&state.pool)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(PlanView {
            id: plan_id,
            version: version.0,
            state: "draft".to_string(),
        }),
    ))
}

async fn accept_plan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, issue_number)): Path<(String, String, i64)>,
) -> Result<StatusCode, AppError> {
    let principal_id = require_session(&state.pool, &headers).await?;
    let (_, repo_id) = repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let updated = sqlx::query(
        "UPDATE implementation_plans p SET state = 'accepted', accepted_at = CURRENT_TIMESTAMP
         FROM issue_revisions r
         JOIN issues i ON i.id = r.issue_id
         WHERE p.issue_revision_id = r.id AND i.repo_id = $1 AND i.number = $2
           AND p.state = 'draft' AND r.id = i.current_revision_id",
    )
    .bind(repo_id)
    .bind(issue_number)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn authenticate_basic(pool: &PgPool, user: &str, pass: &str) -> Result<Uuid, AppError> {
    if pass.starts_with("mhp_") {
        return pat::authenticate_pat(pool, pass).await;
    }
    let _ = user;
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT p.id, p.status, c.password_hash
         FROM principals p
         JOIN user_credentials c ON c.principal_id = p.id
         WHERE p.handle_normalized = $1",
    )
    .bind(crate::auth::normalize_handle(user))
    .fetch_optional(pool)
    .await?;
    let Some((id, status, password_hash)) = row else {
        return Err(AppError::Unauthorized);
    };
    if status != "active" || !verify_password(pass, &password_hash)? {
        return Err(AppError::Unauthorized);
    }
    Ok(id)
}

async fn org_id_from_slug(pool: &PgPool, org_slug: &str) -> Result<Uuid, AppError> {
    let row =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug_normalized = $1")
            .bind(org_slug.to_lowercase())
            .fetch_optional(pool)
            .await?;
    row.map(|(id,)| id).ok_or(AppError::NotFound)
}

pub(crate) async fn org_id_for_member(
    pool: &PgPool,
    org_slug: &str,
    principal_id: Uuid,
) -> Result<Uuid, AppError> {
    let row = sqlx::query_as::<_, (Uuid,)>(
        "SELECT o.id FROM organizations o
         JOIN memberships m ON m.org_id = o.id
         WHERE o.slug_normalized = $1 AND m.principal_id = $2 AND m.status = 'active'",
    )
    .bind(org_slug.to_lowercase())
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    row.map(|(id,)| id).ok_or(AppError::Forbidden)
}

pub(crate) async fn repo_id_for_member(
    pool: &PgPool,
    org_slug: &str,
    repo_slug: &str,
    principal_id: Uuid,
) -> Result<Uuid, AppError> {
    Ok(
        repo_org_ids_for_member(pool, org_slug, repo_slug, principal_id)
            .await?
            .1,
    )
}

#[allow(clippy::type_complexity)]
async fn repo_org_ids_for_member(
    pool: &PgPool,
    org_slug: &str,
    repo_slug: &str,
    principal_id: Uuid,
) -> Result<(Uuid, Uuid), AppError> {
    let org_id = org_id_for_member(pool, org_slug, principal_id).await?;
    let row = sqlx::query_as::<_, (Uuid,)>(
        "SELECT r.id FROM repositories r
         JOIN repository_grants g ON g.repo_id = r.id
         WHERE r.org_id = $1 AND r.slug_normalized = $2 AND g.principal_id = $3",
    )
    .bind(org_id)
    .bind(repo_slug.to_lowercase())
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    let repo_id = row.map(|(id,)| id).ok_or(AppError::Forbidden)?;
    Ok((org_id, repo_id))
}

#[allow(clippy::type_complexity)]
pub(crate) async fn repo_storage_for_member(
    pool: &PgPool,
    org_slug: &str,
    repo_slug: &str,
    principal_id: Uuid,
) -> Result<(Uuid, Uuid, Uuid), AppError> {
    let org_id = org_id_for_member(pool, org_slug, principal_id).await?;
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT r.id, r.storage_key FROM repositories r
         JOIN repository_grants g ON g.repo_id = r.id
         WHERE r.org_id = $1 AND r.slug_normalized = $2 AND g.principal_id = $3",
    )
    .bind(org_id)
    .bind(repo_slug.to_lowercase())
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    let (repo_id, storage_key) = row.ok_or(AppError::Forbidden)?;
    Ok((org_id, repo_id, storage_key))
}

fn normalize_slug(slug: &str) -> Result<String, AppError> {
    let slug = slug.trim().to_lowercase();
    if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(AppError::Validation("invalid repository slug".to_string()));
    }
    Ok(slug)
}
