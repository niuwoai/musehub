//! Opaque list cursors (`created_at` + tie-breaker id), scoped per query kind.

use base64::Engine;
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::error::AppError;

const CURSOR_PREFIX: &str = "mh1:";
const HISTORY_CURSOR_PREFIX: &str = "mhh1:";
const ISSUE_CURSOR_PREFIX: &str = "mhi2:";
pub const MAX_HISTORY_CURSOR_OFFSET: u64 = 100_000;

#[derive(Debug, Deserialize, Serialize)]
struct IssueCursor {
    repo_id: Uuid,
    principal_id: Uuid,
    state: Option<String>,
    assignee_id: Option<Uuid>,
    query: Option<String>,
    label: Option<String>,
    limit: i64,
    number: i64,
    issue_id: Uuid,
}

#[allow(clippy::too_many_arguments)]
pub fn encode_issue_cursor(
    repo_id: Uuid,
    principal_id: Uuid,
    state: Option<&str>,
    assignee_id: Option<Uuid>,
    query: Option<&str>,
    label: Option<&str>,
    limit: i64,
    number: i64,
    issue_id: Uuid,
) -> Result<String, AppError> {
    let payload = IssueCursor {
        repo_id,
        principal_id,
        state: state.map(str::to_owned),
        assignee_id,
        query: query.map(str::to_owned),
        label: label.map(str::to_owned),
        limit,
        number,
        issue_id,
    };
    let bytes = serde_json::to_vec(&payload)
        .map_err(|error| AppError::Other(format!("encode issue cursor: {error}")))?;
    Ok(format!(
        "{ISSUE_CURSOR_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn decode_issue_cursor(
    cursor: &str,
    repo_id: Uuid,
    principal_id: Uuid,
    state: Option<&str>,
    assignee_id: Option<Uuid>,
    query: Option<&str>,
    label: Option<&str>,
    limit: i64,
) -> Result<(i64, Uuid), AppError> {
    let encoded = cursor
        .strip_prefix(ISSUE_CURSOR_PREFIX)
        .ok_or_else(|| AppError::Validation("invalid issue cursor".into()))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AppError::Validation("invalid issue cursor encoding".into()))?;
    let payload: IssueCursor = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Validation("invalid issue cursor payload".into()))?;
    if payload.repo_id != repo_id
        || payload.principal_id != principal_id
        || payload.state.as_deref() != state
        || payload.assignee_id != assignee_id
        || payload.query.as_deref() != query
        || payload.label.as_deref() != label
        || payload.limit != limit
    {
        return Err(AppError::Validation(
            "issue cursor does not match query".into(),
        ));
    }
    Ok((payload.number, payload.issue_id))
}

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
    let raw =
        String::from_utf8(bytes).map_err(|_| AppError::Validation("invalid cursor".into()))?;
    let (ts, id) = raw
        .split_once('|')
        .ok_or_else(|| AppError::Validation("invalid cursor payload".into()))?;
    let created_at = OffsetDateTime::parse(ts, &Rfc3339)
        .map_err(|_| AppError::Validation("invalid cursor timestamp".into()))?;
    let repo_id =
        Uuid::parse_str(id).map_err(|_| AppError::Validation("invalid cursor id".into()))?;
    Ok((created_at, repo_id))
}

pub fn encode_history_cursor(
    repo_id: Uuid,
    principal_id: Uuid,
    commit_oid: &str,
    offset: u64,
    limit: i64,
) -> String {
    let raw = format!("{repo_id}|{principal_id}|{commit_oid}|{offset}|{limit}");
    format!(
        "{HISTORY_CURSOR_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
    )
}

pub fn decode_history_cursor(
    cursor: Option<&str>,
    repo_id: Uuid,
    principal_id: Uuid,
    commit_oid: &str,
    limit: i64,
) -> Result<u64, AppError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let encoded = cursor
        .strip_prefix(HISTORY_CURSOR_PREFIX)
        .ok_or_else(|| AppError::Validation("invalid history cursor".into()))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AppError::Validation("invalid history cursor encoding".into()))?;
    let raw = String::from_utf8(bytes)
        .map_err(|_| AppError::Validation("invalid history cursor".into()))?;
    let fields: Vec<&str> = raw.split('|').collect();
    if fields.len() != 5
        || fields[0] != repo_id.to_string()
        || fields[1] != principal_id.to_string()
        || fields[2] != commit_oid
        || fields[4].parse::<i64>().ok() != Some(limit)
    {
        return Err(AppError::Validation(
            "history cursor does not match query".into(),
        ));
    }
    let offset = fields[3]
        .parse::<u64>()
        .map_err(|_| AppError::Validation("invalid history cursor offset".into()))?;
    if offset >= MAX_HISTORY_CURSOR_OFFSET {
        return Err(AppError::Validation(
            "history cursor offset exceeds limit".into(),
        ));
    }
    Ok(offset)
}

#[cfg(test)]
mod issue_cursor_tests {
    use super::{decode_issue_cursor, encode_issue_cursor};
    use uuid::Uuid;

    #[test]
    fn issue_cursor_round_trips_and_is_bound_to_filters() {
        let repo_id = Uuid::new_v4();
        let principal_id = Uuid::new_v4();
        let assignee_id = Uuid::new_v4();
        let issue_id = Uuid::new_v4();
        let cursor = encode_issue_cursor(
            repo_id,
            principal_id,
            Some("open"),
            Some(assignee_id),
            Some("crash"),
            Some("bug"),
            20,
            42,
            issue_id,
        )
        .expect("encode cursor");

        assert_eq!(
            decode_issue_cursor(
                &cursor,
                repo_id,
                principal_id,
                Some("open"),
                Some(assignee_id),
                Some("crash"),
                Some("bug"),
                20,
            )
            .expect("decode cursor"),
            (42, issue_id)
        );
        assert!(decode_issue_cursor(
            &cursor,
            repo_id,
            principal_id,
            Some("closed"),
            Some(assignee_id),
            Some("crash"),
            Some("bug"),
            20,
        )
        .is_err());
        assert!(decode_issue_cursor(
            &cursor,
            repo_id,
            Uuid::new_v4(),
            Some("open"),
            Some(assignee_id),
            Some("crash"),
            Some("bug"),
            20,
        )
        .is_err());
    }
}
