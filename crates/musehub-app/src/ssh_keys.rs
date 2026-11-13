//! IAM-02: SSH public key management (used by product `musehub-git-ssh` transport).

use std::process::Command;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit;
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
    let line = key.trim();
    if line.is_empty() {
        return Err(AppError::Validation("empty ssh public key".into()));
    }
    if line
        .bytes()
        .any(|b| b == 0 || b == b'\n' || b == b'\r' || (b < 0x20 && b != b'\t'))
    {
        return Err(AppError::Validation(
            "ssh public key must be a single line without control characters".into(),
        ));
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
    let request_id = audit::request_id_from_headers(&headers);
    let name = body.name.trim().to_string();
    let public_key = body.public_key.trim().to_string();
    let metadata = serde_json::json!({ "name": name.as_str(), "fingerprint": fp.as_str() });

    let mut tx = state.pool.begin().await?;
    let insert = sqlx::query(
        "INSERT INTO ssh_keys (id, principal_id, name, public_key, fingerprint)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(principal_id)
    .bind(&name)
    .bind(&canonical)
    .bind(&fp)
    .execute(&mut *tx)
    .await;
    if let Err(e) = insert {
        if let sqlx::Error::Database(db) = &e {
            if db.code().as_deref() == Some("23505") {
                return Err(AppError::Validation("duplicate ssh key fingerprint".into()));
            }
        }
        return Err(e.into());
    }
    audit::record_in_tx(
        &mut tx,
        None,
        None,
        principal_id,
        None,
        "ssh_key.create",
        "ssh_key",
        id,
        request_id,
        "allow",
        metadata,
    )
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(SshKeyView {
            id,
            name,
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
    let request_id = audit::request_id_from_headers(&headers);
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE ssh_keys SET revoked_at = CURRENT_TIMESTAMP WHERE id = $1 AND principal_id = $2 AND revoked_at IS NULL",
    )
    .bind(key_id)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    audit::record_in_tx(
        &mut tx,
        None,
        None,
        principal_id,
        None,
        "ssh_key.revoke",
        "ssh_key",
        key_id,
        request_id,
        "allow",
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
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
    fn accepts_ssh_keygen_pub_trailing_newline() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let priv_path = dir.path().join("id_ed25519");
        let status = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                priv_path.to_str().unwrap(),
                "-N",
                "",
                "-q",
            ])
            .status()
            .expect("ssh-keygen");
        assert!(status.success());
        let raw = std::fs::read_to_string(priv_path.with_extension("pub")).expect("pub");
        assert!(raw.ends_with('\n'));
        assert!(parse_and_canonicalize_ssh_public_key(&raw).is_ok());
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
