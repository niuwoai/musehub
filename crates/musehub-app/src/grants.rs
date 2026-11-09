//! Repository and PAT-scoped grant enforcement.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::pat_auth::PatContext;

pub async fn is_org_privileged(
    pool: &PgPool,
    org_id: Uuid,
    principal_id: Uuid,
) -> Result<bool, AppError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    Ok(matches!(
        row,
        Some((role,)) if role == "owner" || role == "admin"
    ))
}

pub async fn principal_has_repo_grant(
    pool: &PgPool,
    repo_id: Uuid,
    principal_id: Uuid,
) -> Result<bool, AppError> {
    let row = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM repository_grants WHERE repo_id = $1 AND principal_id = $2",
    )
    .bind(repo_id)
    .bind(principal_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0 > 0)
}

pub async fn ensure_principal_repo_access(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    principal_id: Uuid,
) -> Result<(), AppError> {
    if is_org_privileged(pool, org_id, principal_id).await? {
        return Ok(());
    }
    if principal_has_repo_grant(pool, repo_id, principal_id).await? {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

pub async fn ensure_pat_repo_access(
    pool: &PgPool,
    pat: &PatContext,
    org_id: Uuid,
    repo_id: Uuid,
) -> Result<(), AppError> {
    if !crate::pat_auth::pat_org_allowed(pat.org_id, org_id) {
        return Err(AppError::Forbidden);
    }
    let member = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(pat.principal_id)
    .fetch_one(pool)
    .await?;
    if member.0 == 0 {
        return Err(AppError::Forbidden);
    }
    if pat.scope_mode == "selected" {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM token_repository_grants WHERE token_id = $1 AND repo_id = $2",
        )
        .bind(pat.token_id)
        .bind(repo_id)
        .fetch_one(pool)
        .await?;
        if row.0 == 0 {
            return Err(AppError::Forbidden);
        }
        return Ok(());
    }
    if pat.scope_mode == "all_accessible"
        && (crate::pat_auth::scope_allows(&pat.scopes, "pipeline:execute")
            || crate::pat_auth::scope_allows(&pat.scopes, "agent:execute")
            || crate::pat_auth::scope_allows(&pat.scopes, "deploy:execute"))
    {
        return Ok(());
    }
    ensure_principal_repo_access(pool, org_id, repo_id, pat.principal_id).await?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct RepoGrantView {
    pub principal_id: Uuid,
    pub handle: String,
    pub role: String,
}

pub async fn list_repo_grants(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
) -> Result<Vec<RepoGrantView>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT g.principal_id, p.handle, g.role
         FROM repository_grants g
         JOIN principals p ON p.id = g.principal_id
         WHERE g.org_id = $1 AND g.repo_id = $2
         ORDER BY p.handle",
    )
    .bind(org_id)
    .bind(repo_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(principal_id, handle, role)| RepoGrantView {
            principal_id,
            handle,
            role,
        })
        .collect())
}

pub async fn revoke_repo_grant(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    principal_id: Uuid,
) -> Result<bool, AppError> {
    let deleted = sqlx::query(
        "DELETE FROM repository_grants WHERE org_id = $1 AND repo_id = $2 AND principal_id = $3",
    )
    .bind(org_id)
    .bind(repo_id)
    .bind(principal_id)
    .execute(pool)
    .await?;
    Ok(deleted.rows_affected() > 0)
}

pub async fn insert_token_repo_grants(
    pool: &PgPool,
    token_id: Uuid,
    org_id: Uuid,
    repo_ids: &[Uuid],
) -> Result<(), AppError> {
    for repo_id in repo_ids {
        sqlx::query(
            "INSERT INTO token_repository_grants (id, token_id, org_id, repo_id)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (token_id, repo_id) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(token_id)
        .bind(org_id)
        .bind(repo_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}
