use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum_extra::extract::cookie::{Cookie, SameSite};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AppError;

const SESSION_COOKIE: &str = "musehub_session";
const SESSION_TTL_DAYS: i64 = 14;

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    let hash = argon
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| AppError::Other(error.to_string()))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, encoded: &str) -> Result<bool, AppError> {
    let parsed = PasswordHash::new(encoded).map_err(|error| AppError::Other(error.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn session_token() -> String {
    format!("{}{}", Uuid::new_v4(), Uuid::new_v4()).replace('-', "")
}

/// Returns whether a session token appears well-formed (opaque, non-guessable length).
pub fn token_format_valid(token: &str) -> bool {
    token.len() >= 32
}

pub fn hash_token(token: &str) -> Vec<u8> {
    let digest = Sha256::digest(token.as_bytes());
    digest.to_vec()
}

pub async fn login(
    pool: &PgPool,
    handle: &str,
    password: &str,
) -> Result<(Uuid, String), AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT p.id, p.status, c.password_hash
         FROM principals p
         JOIN user_credentials c ON c.principal_id = p.id
         WHERE p.handle_normalized = $1 AND p.kind = 'human'",
    )
    .bind(normalize_handle(handle))
    .fetch_optional(pool)
    .await?;
    let Some((principal_id, status, password_hash)) = row else {
        return Err(AppError::Unauthorized);
    };
    if status != "active" {
        return Err(AppError::Forbidden);
    }
    if !verify_password(password, &password_hash)? {
        return Err(AppError::Unauthorized);
    }
    let token = session_token();
    let token_hash = hash_token(&token);
    let expires = OffsetDateTime::now_utc() + time::Duration::days(SESSION_TTL_DAYS);
    sqlx::query(
        "INSERT INTO sessions (id, principal_id, token_hash, expires_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal_id)
    .bind(token_hash)
    .bind(expires)
    .execute(pool)
    .await?;
    Ok((principal_id, token))
}

pub async fn revoke_all_sessions_for_principal(
    pool: &PgPool,
    principal_id: Uuid,
) -> Result<u64, AppError> {
    let updated = sqlx::query(
        "UPDATE sessions SET revoked_at = CURRENT_TIMESTAMP
         WHERE principal_id = $1 AND revoked_at IS NULL",
    )
    .bind(principal_id)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected())
}

/// Self-service password change: updates hash and revokes all sessions except the current one.
pub async fn change_password(
    pool: &PgPool,
    principal_id: Uuid,
    current_session_token: &str,
    current_password: &str,
    new_password: &str,
) -> Result<(), AppError> {
    crate::security::validate_bootstrap_password(new_password).map_err(AppError::Validation)?;
    let new_hash = hash_password(new_password)?;
    let keep_hash = hash_token(current_session_token);
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT password_hash FROM user_credentials WHERE principal_id = $1 FOR UPDATE",
    )
    .bind(principal_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((password_hash,)) = row else {
        return Err(AppError::Forbidden);
    };
    if !verify_password(current_password, &password_hash)? {
        return Err(AppError::Unauthorized);
    }
    if verify_password(new_password, &password_hash)? {
        return Err(AppError::Validation(
            "new password must differ from current password".to_string(),
        ));
    }
    let updated = sqlx::query(
        "UPDATE user_credentials SET password_hash = $3, password_changed_at = CURRENT_TIMESTAMP
         WHERE principal_id = $1 AND password_hash = $2",
    )
    .bind(principal_id)
    .bind(&password_hash)
    .bind(&new_hash)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "credential changed concurrently; retry with current password".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE sessions SET revoked_at = CURRENT_TIMESTAMP
         WHERE principal_id = $1 AND token_hash != $2 AND revoked_at IS NULL",
    )
    .bind(principal_id)
    .bind(&keep_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn logout(pool: &PgPool, token: &str) -> Result<(), AppError> {
    let token_hash = hash_token(token);
    sqlx::query(
        "UPDATE sessions SET revoked_at = CURRENT_TIMESTAMP
         WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn principal_from_session(pool: &PgPool, token: &str) -> Result<Uuid, AppError> {
    if !token_format_valid(token) {
        return Err(AppError::Unauthorized);
    }
    let token_hash = hash_token(token);
    let row = sqlx::query_as::<_, (Uuid, OffsetDateTime, Option<OffsetDateTime>, String)>(
        "SELECT s.principal_id, s.expires_at, s.revoked_at, p.status
         FROM sessions s
         JOIN principals p ON p.id = s.principal_id
         WHERE s.token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?;
    let Some((principal_id, expires_at, revoked_at, status)) = row else {
        return Err(AppError::Unauthorized);
    };
    if revoked_at.is_some() || expires_at < OffsetDateTime::now_utc() || status != "active" {
        return Err(AppError::Unauthorized);
    }
    let _ =
        sqlx::query("UPDATE sessions SET last_seen_at = CURRENT_TIMESTAMP WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(pool)
            .await;
    Ok(principal_id)
}

pub fn session_cookie(token: &str) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(false)
        .max_age(time::Duration::days(SESSION_TTL_DAYS))
        .build()
}

pub fn clear_session_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(0))
        .build()
}

pub fn cookie_name() -> &'static str {
    SESSION_COOKIE
}

pub fn normalize_handle(handle: &str) -> String {
    handle.trim().to_lowercase()
}
