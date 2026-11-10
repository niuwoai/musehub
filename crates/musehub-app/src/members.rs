//! Org member administration (no public self-registration).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
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

fn parse_org_role_strict(role: &str) -> Result<&'static str, AppError> {
    match role {
        "member" => Ok("member"),
        "admin" => Ok("admin"),
        "owner" => Ok("owner"),
        other => Err(AppError::Validation(format!("invalid role: {other}"))),
    }
}

async fn principal_id_for_handle(pool: &sqlx::PgPool, handle: &str) -> Result<Uuid, AppError> {
    let normalized = normalize_handle(handle);
    let row =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = $1")
            .bind(normalized)
            .fetch_optional(pool)
            .await?;
    row.map(|(id,)| id).ok_or(AppError::NotFound)
}

async fn actor_org_role(pool: &sqlx::PgPool, org_id: Uuid, actor_id: Uuid) -> Result<String, AppError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(actor_id)
    .fetch_optional(pool)
    .await?;
    row.map(|(r,)| r).ok_or(AppError::Forbidden)
}

async fn count_active_owners(pool: &sqlx::PgPool, org_id: Uuid) -> Result<i64, AppError> {
    let row = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM memberships m
         JOIN principals p ON p.id = m.principal_id
         WHERE m.org_id = $1 AND m.role = 'owner' AND m.status = 'active' AND p.status = 'active'",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

#[derive(Serialize)]
pub struct OrgMemberView {
    pub principal_id: Uuid,
    pub handle: String,
    pub org_role: String,
    pub principal_status: String,
    pub membership_status: String,
}

pub async fn list_org_members(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Json<Vec<OrgMemberView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, principal_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, String)>(
        "SELECT p.id, p.handle, m.role, p.status, m.status
         FROM memberships m
         JOIN principals p ON p.id = m.principal_id
         WHERE m.org_id = $1
         ORDER BY p.handle",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(principal_id, handle, org_role, principal_status, membership_status)| {
                    OrgMemberView {
                        principal_id,
                        handle,
                        org_role,
                        principal_status,
                        membership_status,
                    }
                },
            )
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct PatchMemberRequest {
    pub role: String,
}

pub async fn patch_org_member(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, handle)): Path<(String, String)>,
    Json(body): Json<PatchMemberRequest>,
) -> Result<StatusCode, AppError> {
    let actor_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, actor_id).await?;
    let target_id = principal_id_for_handle(&state.pool, &handle).await?;
  let new_role = parse_org_role_strict(&body.role)?;
    let actor_role = actor_org_role(&state.pool, org_id, actor_id).await?;
    if new_role == "owner" && actor_role != "owner" {
        return Err(AppError::Forbidden);
    }
    if actor_role == "admin" && new_role != "member" {
        return Err(AppError::Forbidden);
    }
    if target_id == actor_id {
        return Err(AppError::Validation("cannot change your own org role".to_string()));
    }
    let current = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(target_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((current_role,)) = current else {
        return Err(AppError::NotFound);
    };
    if current_role == "owner" && new_role != "owner" && count_active_owners(&state.pool, org_id).await? <= 1 {
        return Err(AppError::Validation(
            "cannot demote the last org owner".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE memberships SET role = $3 WHERE org_id = $1 AND principal_id = $2",
    )
    .bind(org_id)
    .bind(target_id)
    .bind(new_role)
    .execute(&state.pool)
    .await?;
    let request_id = crate::audit::request_id_from_headers(&headers);
    let _ = crate::audit::record(
        &state.pool,
        Some(org_id),
        None,
        actor_id,
        None,
        "org.member.role.update",
        "membership",
        target_id,
        request_id,
        "allow",
        serde_json::json!({
            "handle": handle,
            "from_role": current_role,
            "to_role": new_role,
        }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn disable_org_member(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, handle)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let actor_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, actor_id).await?;
    let target_id = principal_id_for_handle(&state.pool, &handle).await?;
    if target_id == actor_id {
        return Err(AppError::Validation("cannot disable yourself".to_string()));
    }
    let membership = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(target_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((role,)) = membership else {
        return Err(AppError::NotFound);
    };
    if role == "owner" && count_active_owners(&state.pool, org_id).await? <= 1 {
        return Err(AppError::Validation(
            "cannot disable the last org owner".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE principals SET status = 'disabled', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(target_id)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE memberships SET status = 'inactive' WHERE org_id = $1 AND principal_id = $2",
    )
    .bind(org_id)
    .bind(target_id)
    .execute(&state.pool)
    .await?;
    let _ = crate::auth::revoke_all_sessions_for_principal(&state.pool, target_id).await?;
    let request_id = crate::audit::request_id_from_headers(&headers);
    let _ = crate::audit::record(
        &state.pool,
        Some(org_id),
        None,
        actor_id,
        None,
        "org.member.disable",
        "principal",
        target_id,
        request_id,
        "allow",
        serde_json::json!({ "handle": handle }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn revoke_member_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, handle)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let actor_id = super::require_session(&state.pool, &headers).await?;
    let org_id = require_org_admin(&state.pool, &org, actor_id).await?;
    let target_id = principal_id_for_handle(&state.pool, &handle).await?;
    let active = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(target_id)
    .fetch_one(&state.pool)
    .await?;
    if active.0 == 0 {
        return Err(AppError::NotFound);
    }
    let revoked = crate::auth::revoke_all_sessions_for_principal(&state.pool, target_id).await?;
    let request_id = crate::audit::request_id_from_headers(&headers);
    let _ = crate::audit::record(
        &state.pool,
        Some(org_id),
        None,
        actor_id,
        None,
        "org.member.sessions.revoke",
        "principal",
        target_id,
        request_id,
        "allow",
        serde_json::json!({ "handle": handle, "sessions_revoked": revoked }),
    )
    .await;
    Ok(Json(serde_json::json!({ "sessions_revoked": revoked })))
}

pub(crate) async fn require_org_admin(
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
    let actor_role = actor_org_role(&state.pool, org_id, principal_id).await?;
    let handle = body.handle.trim();
    if handle.is_empty() {
        return Err(AppError::Validation("handle required".to_string()));
    }
    validate_bootstrap_password(&body.password).map_err(AppError::Validation)?;
    let role = parse_role(body.role)?;
    if role == "admin" && actor_role != "owner" {
        return Err(AppError::Forbidden);
    }
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
    let request_id = crate::audit::request_id_from_headers(&headers);
    let _ = crate::audit::record(
        &state.pool,
        Some(org_id),
        None,
        principal_id,
        None,
        "org.member.invite",
        "membership",
        member_id,
        request_id,
        "allow",
        serde_json::json!({ "handle": handle, "role": role }),
    )
    .await;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
pub struct GrantRepoRequest {
    pub handle: String,
    pub role: Option<String>,
}

async fn grantor_org_role(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    principal_id: Uuid,
) -> Result<String, AppError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(principal_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

fn role_rank(role: &str) -> u8 {
    match role {
        "reader" => 1,
        "developer" => 2,
        "maintainer" => 3,
        _ => 0,
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
    if member_id == principal_id {
        return Err(AppError::Validation(
            "cannot grant repository access to yourself".to_string(),
        ));
    }
    let mut role = body.role.clone();
    if role.is_none() {
        let prefs = crate::org_config::load_preferences(&state.pool, org_id).await?;
        role = Some(prefs.default_collaborator_role);
    }
    let role = crate::org_config::parse_repo_role(role)?;
    let grantor = grantor_org_role(&state.pool, org_id, principal_id).await?;
    if role == "maintainer" && grantor != "owner" {
        return Err(AppError::Forbidden);
    }
    let member_org_role = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(member_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((member_org_role,)) = member_org_role else {
        return Err(AppError::Validation(
            "target is not an org member".to_string(),
        ));
    };
    if role_rank(role) > role_rank(&grantor) && grantor != "owner" {
        return Err(AppError::Forbidden);
    }
    if role_rank(role) > role_rank(&member_org_role) + 1 && grantor != "owner" {
        return Err(AppError::Validation(
            "cannot elevate beyond membership".to_string(),
        ));
    }
    grant_repo_access(&state.pool, org_id, repo_id, member_id, role).await?;
    let request_id = crate::audit::request_id_from_headers(&headers);
    let _ = crate::audit::record(
        &state.pool,
        Some(org_id),
        Some(repo_id),
        principal_id,
        None,
        "repository.grant",
        "repository_grant",
        repo_id,
        request_id,
        "allow",
        serde_json::json!({
            "principal_id": member_id,
            "role": role,
        }),
    )
    .await;
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
    let removed =
        crate::grants::revoke_repo_grant(&state.pool, org_id, repo_id, query.principal_id).await?;
    if removed {
        let request_id = crate::audit::request_id_from_headers(&headers);
        let _ = crate::audit::record(
            &state.pool,
            Some(org_id),
            Some(repo_id),
            principal_id,
            None,
            "repository.grant.revoke",
            "repository_grant",
            repo_id,
            request_id,
            "allow",
            serde_json::json!({ "principal_id": query.principal_id }),
        )
        .await;
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
