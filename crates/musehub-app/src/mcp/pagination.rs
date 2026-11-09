//! Opaque list cursors (`created_at` + tie-breaker id), scoped per query kind.

use base64::Engine;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::error::AppError;

const CURSOR_PREFIX: &str = "mh1:";

pub fn encode_repo_cursor(created_at: OffsetDateTime, repo_id: Uuid) -> String {
    let ts = created_at
        .format(&Rfc3339)
        .unwrap_or_else(|_| created_at.to_string());
    let raw = format!("{ts}|{repo_id}");
    format!(
        "{CURSOR_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
    )
}

pub fn decode_repo_cursor(cursor: &str) -> Result<(OffsetDateTime, Uuid), AppError> {
    let encoded = cursor
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| AppError::Validation("invalid cursor".into()))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AppError::Validation("invalid cursor encoding".into()))?;
    let raw = String::from_utf8(bytes).map_err(|_| AppError::Validation("invalid cursor".into()))?;
    let (ts, id) = raw
        .split_once('|')
        .ok_or_else(|| AppError::Validation("invalid cursor payload".into()))?;
    let created_at =
        OffsetDateTime::parse(ts, &Rfc3339).map_err(|_| AppError::Validation("invalid cursor timestamp".into()))?;
    let repo_id = Uuid::parse_str(id).map_err(|_| AppError::Validation("invalid cursor id".into()))?;
    Ok((created_at, repo_id))
}
