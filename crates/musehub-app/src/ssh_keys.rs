//! IAM-02: SSH public key management (Git-over-SSH transport not productized here).

use std::process::Command;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::AppState;

#[derive(Serialize)]
pub struct SshKeyView {
    pub id: Uuid,
    pub name: String,
    pub fingerprint: String,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct CreateSshKeyRequest {
    pub name: String,
    pub public_key: String,
}

fn validate_public_key(key: &str) -> Result<(), AppError> {
    let trimmed = key.trim();
    let kind = trimmed.split_whitespace().next().unwrap_or("");
    if !matches!(
        kind,
        "ssh-ed25519"
            | "ssh-rsa"
            | "ecdsa-sha2-nistp256"
            | "ecdsa-sha2-nistp384"
            | "ecdsa-sha2-nistp521"
    ) {
        return Err(AppError::Validation(
            "unsupported or invalid ssh public key type".into(),
        ));
    }
    if trimmed.split_whitespace().count() < 2 {
        return Err(AppError::Validation("invalid ssh public key format".into()));
    }
    Ok(())
}

fn fingerprint_public_key(key: &str) -> Result<String, AppError> {
    use std::io::Write;
    let mut child = Command::new("ssh-keygen")
        .args(["-lf", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AppError::Other(e.to_string()))?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| AppError::Other("ssh-keygen stdin".into()))?;
        stdin
            .write_all(key.trim().as_bytes())
            .map_err(|e| AppError::Other(e.to_string()))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !out.status.success() {
        return Err(AppError::Validation("could not fingerprint ssh key".into()));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let fp = line.split_whitespace().nth(1).unwrap_or("").to_string();
    if fp.is_empty() {
        return Err(AppError::Validation("could not fingerprint ssh key".into()));
    }
    Ok(fp)
}

pub async fn list_ssh_keys(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<SshKeyView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, time::OffsetDateTime)>(
        "SELECT id, name, fingerprint, created_at FROM ssh_keys
         WHERE principal_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC",
    )
    .bind(principal_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, name, fingerprint, created_at)| SshKeyView {
                id,
                name,
                fingerprint,
                created_at: created_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            })
            .collect(),
    ))
}

pub async fn create_ssh_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateSshKeyRequest>,
) -> Result<(StatusCode, Json<SshKeyView>), AppError> {
    if body.name.trim().is_empty() {
        return Err(AppError::Validation("name required".into()));
    }
    validate_public_key(&body.public_key)?;
    let fp = fingerprint_public_key(&body.public_key)?;
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let id = Uuid::new_v4();
    let result = sqlx::query(
        "INSERT INTO ssh_keys (id, principal_id, name, public_key, fingerprint)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(principal_id)
    .bind(body.name.trim())
    .bind(body.public_key.trim())
    .bind(&fp)
    .execute(&state.pool)
    .await;
    if let Err(e) = result {
        if let sqlx::Error::Database(db) = &e {
            if db.code().as_deref() == Some("23505") {
                return Err(AppError::Validation("duplicate ssh key fingerprint".into()));
            }
        }
        return Err(e.into());
    }
    Ok((
        StatusCode::CREATED,
        Json(SshKeyView {
            id,
            name: body.name.trim().to_string(),
            fingerprint: fp,
            created_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        }),
    ))
}

pub async fn revoke_ssh_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(key_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let updated = sqlx::query(
        "UPDATE ssh_keys SET revoked_at = CURRENT_TIMESTAMP WHERE id = $1 AND principal_id = $2 AND revoked_at IS NULL",
    )
    .bind(key_id)
    .bind(principal_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
