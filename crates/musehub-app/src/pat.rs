use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::token_format_valid;
use crate::error::AppError;

pub fn hash_pat(token: &str) -> Vec<u8> {
    let digest = Sha256::digest(token.as_bytes());
    digest.to_vec()
}

pub fn generate_pat() -> String {
    format!("mhp_{}{}", Uuid::new_v4(), Uuid::new_v4()).replace('-', "")
}

pub async fn authenticate_pat(pool: &PgPool, token: &str) -> Result<Uuid, AppError> {
    if !token.starts_with("mhp_") || !token_format_valid(token) {
        return Err(AppError::Unauthorized);
    }
    let token_hash = hash_pat(token);
    let row = sqlx::query_as::<_, (Uuid, OffsetDateTime, Option<OffsetDateTime>)>(
        "SELECT principal_id, expires_at, revoked_at FROM access_tokens WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?;
    let Some((principal_id, expires_at, revoked_at)) = row else {
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
    Ok(principal_id)
}

pub async fn create_pat_with_scopes(
    pool: &PgPool,
    principal_id: Uuid,
    org_id: Option<Uuid>,
    name: &str,
    scopes: &[&str],
) -> Result<(Uuid, String), AppError> {
    let plain = generate_pat();
    let token_hash = hash_pat(&plain);
    let prefix: String = plain.chars().take(12).collect();
    let id = Uuid::new_v4();
    let expires = OffsetDateTime::now_utc() + time::Duration::days(90);
    sqlx::query(
        "INSERT INTO access_tokens (id, principal_id, org_id, name, token_prefix, token_hash, scopes, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7::text[], $8)",
    )
    .bind(id)
    .bind(principal_id)
    .bind(org_id)
    .bind(name)
    .bind(prefix)
    .bind(token_hash)
    .bind(scopes)
    .bind(expires)
    .execute(pool)
    .await?;
    Ok((id, plain))
}

pub async fn create_pat(
    pool: &PgPool,
    principal_id: Uuid,
    org_id: Option<Uuid>,
    name: &str,
) -> Result<(Uuid, String), AppError> {
    create_pat_with_scopes(
        pool,
        principal_id,
        org_id,
        name,
        &["repo:read", "repo:write"],
    )
    .await
}
