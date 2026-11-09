//! Org member administration (no public self-registration).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{hash_password, normalize_handle};
use crate::error::AppError;
use crate::security::validate_bootstrap_password;
use crate::AppState;

#[derive(Deserialize)]
pub struct InviteMemberRequest {
    pub handle: String,
    pub password: String,
    pub role: Option<String>,
}

fn parse_role(role: Option<String>) -> Result<&'static str, AppError> {
    match role.as_deref().unwrap_or("member") {
        "member" => Ok("member"),
        "admin" => Ok("admin"),
        other => Err(AppError::Validation(format!("invalid role: {other}"))),
    }
}

async fn require_org_admin(
    pool: &sqlx::PgPool,
    org_slug: &str,
    principal_id: Uuid,
) -> Result<Uuid, AppError> {
    let org_id = super::org_id_for_member(pool, org_slug, principal_id).await?;
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    let Some((role,)) = row else {
        return Err(AppError::Forbidden);
    };
    if role != "owner" && role != "admin" {
        return Err(AppError::Forbidden);
    }
    Ok(org_id)
}

/// Invite or create a principal and add org membership (admin/owner only).
pub async fn invite_member(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<InviteMemberRequest>,
) -> Result<StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, principal_id).await?;
    let handle = body.handle.trim();
    if handle.is_empty() {
        return Err(AppError::Validation("handle required".to_string()));
    }
    validate_bootstrap_password(&body.password).map_err(AppError::Validation)?;
    let role = parse_role(body.role)?;
    let normalized = normalize_handle(handle);
    let existing =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = $1")
            .bind(&normalized)
            .fetch_optional(&state.pool)
            .await?;
    let member_id = if let Some((id,)) = existing {
        id
    } else {
        let id = Uuid::new_v4();
        let password_hash = hash_password(&body.password)?;
        let mut tx = state.pool.begin().await?;
        sqlx::query(
            "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
             VALUES ($1, 'human', $2, $3, $2, 'active')",
        )
        .bind(id)
        .bind(handle)
        .bind(&normalized)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO user_credentials (id, principal_id, password_hash, password_changed_at)
             VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(password_hash)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        id
    };
    let inserted = sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status)
         VALUES ($1, $2, $3, $4, 'active')
         ON CONFLICT (org_id, principal_id) DO UPDATE SET role = EXCLUDED.role, status = 'active'",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(member_id)
    .bind(role)
    .execute(&state.pool)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(AppError::Other("membership insert failed".to_string()));
    }
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
pub struct GrantRepoRequest {
    pub handle: String,
    pub role: Option<String>,
}

fn parse_repo_role(role: Option<String>) -> Result<&'static str, AppError> {
    match role.as_deref().unwrap_or("reader") {
        "reader" => Ok("reader"),
        "developer" => Ok("developer"),
        "maintainer" => Ok("maintainer"),
        other => Err(AppError::Validation(format!("invalid repo role: {other}"))),
    }
}

pub async fn grant_repo_collaborator(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Json(body): Json<GrantRepoRequest>,
) -> Result<StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, principal_id).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let normalized = normalize_handle(&body.handle);
    let member =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = $1")
            .bind(normalized)
            .fetch_optional(&state.pool)
            .await?;
    let Some((member_id,)) = member else {
        return Err(AppError::NotFound);
    };
    let role = parse_repo_role(body.role)?;
    grant_repo_access(&state.pool, org_id, repo_id, member_id, role).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_repo_grants(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
) -> Result<Json<Vec<crate::grants::RepoGrantView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, principal_id).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let grants = crate::grants::list_repo_grants(&state.pool, org_id, repo_id).await?;
    Ok(Json(grants))
}

#[derive(Deserialize)]
pub struct RevokeGrantQuery {
    pub principal_id: Uuid,
}

pub async fn revoke_repo_grant(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Query(query): Query<RevokeGrantQuery>,
) -> Result<StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, principal_id).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let removed = crate::grants::revoke_repo_grant(
        &state.pool,
        org_id,
        repo_id,
        query.principal_id,
    )
    .await?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

pub async fn grant_repo_access(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    principal_id: Uuid,
    role: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (repo_id, principal_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(principal_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}
