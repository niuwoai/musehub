//! Validates PR review comment anchors against the stored base/head diff.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::AppError;
use crate::mcp::git_read::{git_commit_compare, git_unified_diff};

const MAX_DIFF_BYTES: usize = 512 * 1024;

pub fn validate_and_normalize_pr_anchor(
    bare: &Path,
    base_oid: &str,
    head_oid: &str,
    row_version: i64,
    anchor: Value,
) -> Result<Value, AppError> {
    let obj = anchor
        .as_object()
        .ok_or_else(|| AppError::Validation("anchor must be an object".into()))?;
    let path = obj
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Validation("anchor.path required".into()))?;
    let line = obj
        .get("line")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| AppError::Validation("anchor.line required".into()))?;
    if line == 0 {
        return Err(AppError::Validation("anchor.line invalid".into()));
    }
    let side = obj
        .get("side")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Validation("anchor.side required".into()))?;
    let side_norm = side.to_ascii_uppercase();
    if side_norm != "LEFT" && side_norm != "RIGHT" {
        return Err(AppError::Validation(
            "anchor.side must be LEFT or RIGHT".into(),
        ));
    }
    let client_rev = obj
        .get("pr_revision_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| AppError::Validation("anchor.pr_revision_id required".into()))?;
    if client_rev != row_version {
        return Err(AppError::Validation("stale pull request revision".into()));
    }

    let (files, _, _) = git_commit_compare(bare, base_oid, head_oid, None, 10_000)?;
    let in_diff = files.iter().any(|f| {
        f.get("new_path").and_then(|v| v.as_str()) == Some(path)
            || f.get("old_path").and_then(|v| v.as_str()) == Some(path)
    });
    if !in_diff {
        return Err(AppError::Validation("path not in pull diff".into()));
    }

    let diff = git_unified_diff(bare, base_oid, head_oid, path, MAX_DIFF_BYTES)?;
    if diff.get("binary").and_then(|v| v.as_bool()) == Some(true) {
        return Err(AppError::Validation("cannot anchor on binary file".into()));
    }
    let unified = diff.get("unified").and_then(|v| v.as_str()).unwrap_or("");
    if !line_exists_in_unified(unified, line, &side_norm) {
        return Err(AppError::Validation("line not in diff hunk".into()));
    }

    Ok(json!({
        "path": path,
        "line": line,
        "side": side_norm,
        "pr_revision_id": row_version,
        "base_oid": base_oid,
        "head_oid": head_oid,
    }))
}

fn parse_hunk_starts(line: &str) -> Option<(u64, u64)> {
    let rest = line.strip_prefix("@@")?.trim();
    let mut parts = rest.split_whitespace();
    let old_part = parts.next()?;
    let new_part = parts.next()?;
    let old_start = old_part
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse::<u64>()
        .ok()?;
    let new_start = new_part
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse::<u64>()
        .ok()?;
    Some((old_start, new_start))
}

fn line_exists_in_unified(unified: &str, target_line: u64, side: &str) -> bool {
    let mut old_line = 0u64;
    let mut new_line = 0u64;
    let mut in_hunk = false;
    for line in unified.lines() {
        if let Some((old_start, new_start)) = parse_hunk_starts(line) {
            old_line = old_start;
            new_line = new_start;
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        if line.starts_with("\\") {
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            if side == "RIGHT" && new_line == target_line {
                return true;
            }
            new_line = new_line.saturating_add(1);
        } else if line.starts_with('-') {
            if side == "LEFT" && old_line == target_line {
                return true;
            }
            old_line = old_line.saturating_add(1);
        } else if line.starts_with(' ') || line.is_empty() {
            if side == "LEFT" && old_line == target_line {
                return true;
            }
            if side == "RIGHT" && new_line == target_line {
                return true;
            }
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
        }
    }
    false
}
