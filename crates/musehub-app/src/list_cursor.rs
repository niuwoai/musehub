//! Opaque list cursors for REST pagination.

use base64::Engine;
use uuid::Uuid;

use crate::error::AppError;

const CURSOR_PREFIX: &str = "mhi1:";

pub fn encode_issue_cursor(number: i64, id: Uuid) -> String {
    let raw = format!("{number}|{id}");
    format!(
        "{CURSOR_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
    )
}

pub fn decode_issue_cursor(cursor: &str) -> Result<(i64, Uuid), AppError> {
    let encoded = cursor
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| AppError::Validation("invalid cursor".into()))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AppError::Validation("invalid cursor encoding".into()))?;
    let raw =
        String::from_utf8(bytes).map_err(|_| AppError::Validation("invalid cursor".into()))?;
    let (num, id) = raw
        .split_once('|')
        .ok_or_else(|| AppError::Validation("invalid cursor payload".into()))?;
    let number: i64 = num
        .parse()
        .map_err(|_| AppError::Validation("invalid cursor number".into()))?;
    let issue_id =
        Uuid::parse_str(id).map_err(|_| AppError::Validation("invalid cursor id".into()))?;
    Ok((number, issue_id))
}
