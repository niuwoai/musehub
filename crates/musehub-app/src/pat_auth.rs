//! PAT authentication context for scope and org enforcement.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AppError;
use crate::pat;

#[derive(Debug, Clone)]
pub struct PatContext {
    pub principal_id: Uuid,
    pub org_id: Option<Uuid>,
    pub scopes: Vec<String>,
}

pub async fn authenticate_pat_context(pool: &PgPool, token: &str) -> Result<PatContext, AppError> {
    if !token.starts_with("mhp_") {
        return Err(AppError::Unauthorized);
    }
    let token_hash = pat::hash_pat(token);
    let row = sqlx::query_as::<_, (Uuid, Option<Uuid>, Vec<String>, OffsetDateTime, Option<OffsetDateTime>)>(
        "SELECT principal_id, org_id, scopes, expires_at, revoked_at FROM access_tokens WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?;
    let Some((principal_id, org_id, scopes, expires_at, revoked_at)) = row else {
        return Err(AppError::Unauthorized);
    };
    if revoked_at.is_some() || expires_at < OffsetDateTime::now_utc() {
        return Err(AppError::Unauthorized);
    }
    let _ = sqlx::query(
        "UPDATE access_tokens SET last_used_at = CURRENT_TIMESTAMP WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .execute(pool)
    .await;
    Ok(PatContext {
        principal_id,
        org_id,
        scopes,
    })
}

pub fn scope_allows(scopes: &[String], required: &str) -> bool {
    scopes
        .iter()
        .any(|scope| scope == required || scope == "repo:admin")
}

pub fn pat_org_allowed(pat_org: Option<Uuid>, resource_org: Uuid) -> bool {
    match pat_org {
        None => true,
        Some(id) => id == resource_org,
    }
}
