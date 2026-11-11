//! IAM-02: SSH public key management (used by product `musehub-git-ssh` transport).

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

const ALLOWED_KEY_TYPES: &[&str] = &[
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
];

/// Rejects multiline / control-char payloads before parse, fingerprint, store, or AKC output.
pub fn parse_and_canonicalize_ssh_public_key(key: &str) -> Result<String, AppError> {
    if key.is_empty() {
        return Err(AppError::Validation("empty ssh public key".into()));
    }
    if key
        .bytes()
        .any(|b| b == 0 || b == b'\n' || b == b'\r' || (b < 0x20 && b != b'\t'))
    {
        return Err(AppError::Validation(
            "ssh public key must be a single line without control characters".into(),
        ));
    }
    let line = key.trim();
    if line.is_empty() {
        return Err(AppError::Validation("empty ssh public key".into()));
    }
    if line.contains('\\') {
        return Err(AppError::Validation("invalid ssh public key".into()));
    }
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 2 || tokens.len() > 3 {
        return Err(AppError::Validation(
            "ssh public key must contain exactly one key (type, base64, optional comment)".into(),
        ));
    }
    let kind = tokens[0];
    if !ALLOWED_KEY_TYPES.contains(&kind) {
        return Err(AppError::Validation(
            "unsupported or invalid ssh public key type".into(),
        ));
    }
    let body = tokens[1];
    validate_base64_key_body(kind, body)?;
    let canonical = if let Some(comment) = tokens.get(2) {
        let c = validate_ssh_key_comment(comment)?;
        format!("{kind} {body} {c}")
    } else {
        format!("{kind} {body}")
    };
    Ok(canonical)
}

/// Fail-closed re-validation for keys already stored (malformed rows never reach AKC).
pub fn sanitize_stored_ssh_public_key(stored: &str) -> Result<String, AppError> {
    parse_and_canonicalize_ssh_public_key(stored)
}

fn validate_ssh_key_comment(comment: &str) -> Result<String, AppError> {
    if comment.is_empty() || comment.len() > 255 {
        return Err(AppError::Validation("invalid ssh key comment".into()));
    }
    if comment
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\'' || c == '\\' || c == '\n' || c == '\r')
    {
        return Err(AppError::Validation("invalid ssh key comment".into()));
    }
    Ok(comment.to_string())
}

fn validate_base64_key_body(kind: &str, body: &str) -> Result<(), AppError> {
    if body.is_empty() || body.len() > 16 * 1024 {
        return Err(AppError::Validation(
            "invalid ssh public key encoding".into(),
        ));
    }
    if !body
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
    {
        return Err(AppError::Validation(
            "invalid ssh public key encoding".into(),
        ));
    }
    match kind {
        "ssh-ed25519" if body.len() >= 40 => Ok(()),
        "ssh-rsa" if body.len() >= 80 => Ok(()),
        "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521"
            if body.len() >= 40 =>
        {
            Ok(())
        }
        _ => Err(AppError::Validation(
            "invalid ssh public key encoding".into(),
        )),
    }
}

pub fn fingerprint_public_key_canonical(canonical: &str) -> Result<String, AppError> {
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
            .write_all(canonical.as_bytes())
            .map_err(|e| AppError::Other(e.to_string()))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| AppError::Other(e.to_string()))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !out.status.success() {
        return Err(AppError::Validation("could not fingerprint ssh key".into()));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let first = line.lines().next().unwrap_or("");
    let fp = first.split_whitespace().nth(1).unwrap_or("").to_string();
    if fp.is_empty() {
        return Err(AppError::Validation("could not fingerprint ssh key".into()));
    }
    if line.lines().count() > 1 {
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
    let canonical = parse_and_canonicalize_ssh_public_key(&body.public_key)?;
    let fp = fingerprint_public_key_canonical(&canonical)?;
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let id = Uuid::new_v4();
    let result = sqlx::query(
        "INSERT INTO ssh_keys (id, principal_id, name, public_key, fingerprint)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(principal_id)
    .bind(body.name.trim())
    .bind(&canonical)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ed25519() -> String {
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICXd1mIaYHo4ZxmswgaeWTij03jgeXse1S6HqIvQ1jrs test@musehub"
            .to_string()
    }

    #[test]
    fn rejects_multiline_second_key_line() {
        let injected = format!(
            "{}\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIINJECTION no-forced-command",
            sample_ed25519()
        );
        assert!(parse_and_canonicalize_ssh_public_key(&injected).is_err());
    }

    #[test]
    fn rejects_control_characters() {
        let mut bad = sample_ed25519();
        bad.push('\0');
        assert!(parse_and_canonicalize_ssh_public_key(&bad).is_err());
    }

    #[test]
    fn canonicalizes_single_line() {
        let c = parse_and_canonicalize_ssh_public_key(&sample_ed25519()).unwrap();
        assert!(!c.contains('\n'));
        assert!(c.starts_with("ssh-ed25519 "));
    }

    #[test]
    fn fingerprint_uses_single_key_only() {
        let c = parse_and_canonicalize_ssh_public_key(&sample_ed25519()).unwrap();
        let fp = fingerprint_public_key_canonical(&c).unwrap();
        assert!(fp.starts_with("SHA256:"));
    }
}
