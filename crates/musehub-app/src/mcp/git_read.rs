//! Git read helpers for MCP repository/PR tools.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::AppError;
use crate::security::validate_git_oid;

pub fn validate_repo_path(path: &str) -> Result<(), AppError> {
    if path.contains("..") || path.starts_with('/') {
        return Err(AppError::Validation("invalid path".to_string()));
    }
    Ok(())
}

/// Rejects empty paths (blob/diff); tree root uses `validate_repo_path("")`.
pub fn validate_repo_path_non_empty(path: &str) -> Result<(), AppError> {
    validate_repo_path(path)?;
    if path.is_empty() {
        return Err(AppError::Validation("invalid path".to_string()));
    }
    Ok(())
}

const MAX_GIT_CMD_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

fn git_command_output_limited(args: &[&str], max_bytes: usize) -> Result<Vec<u8>, AppError> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new("git")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Other(e.to_string()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Other("git stdout".into()))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        let n = stdout
            .read(&mut chunk)
            .map_err(|e| AppError::Other(e.to_string()))?;
        if n == 0 {
            break;
        }
        if buf.len() + n > max_bytes {
            let _ = child.kill();
            return Err(AppError::Validation("git output too large".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let status = child.wait().map_err(|e| AppError::Other(e.to_string()))?;
    if !status.success() {
        return Err(AppError::NotFound);
    }
    Ok(buf)
}

pub fn git_tree_entries(
    bare: &Path,
    commit_oid: &str,
    path: &str,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    validate_git_oid(commit_oid).map_err(AppError::Validation)?;
    validate_repo_path(path)?;
    let tree_path = if path.is_empty() {
        commit_oid
    } else {
        &format!("{commit_oid}:{path}")
    };
    let output = std::process::Command::new("git")
        .args([
            "--git-dir",
            bare.to_str()
                .ok_or_else(|| AppError::Other("git path".into()))?,
            "ls-tree",
            "-l",
            tree_path,
        ])
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(AppError::NotFound);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let mode = parts[0];
        let kind = match mode.chars().next() {
            Some('1') => "blob",
            Some('4') => "tree",
            Some('2') => "blob",
            Some('3') => "symlink",
            _ => "unknown",
        };
        if kind == "symlink" {
            continue;
        }
        let oid = parts[2];
        let size = parts[3].parse::<i64>().ok();
        let name = parts[4..].join(" ");
        let entry_path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        items.push(json!({
            "path": entry_path,
            "kind": kind,
            "oid": oid,
            "size": size,
        }));
        if items.len() as i64 >= limit {
            break;
        }
    }
    let has_more = stdout.lines().filter(|l| !l.trim().is_empty()).count() > items.len();
    Ok((items, has_more))
}

pub fn git_read_file_at(
    bare: &Path,
    commit_oid: &str,
    path: &str,
    start_line: u32,
    max_lines: u32,
) -> Result<Value, AppError> {
    validate_git_oid(commit_oid).map_err(AppError::Validation)?;
    validate_repo_path_non_empty(path)?;
    let spec = format!("{commit_oid}:{path}");
    let git_dir = bare
        .to_str()
        .ok_or_else(|| AppError::Other("git path".into()))?;
    let bytes = git_command_output_limited(
        &["--git-dir", git_dir, "show", &spec],
        MAX_GIT_CMD_OUTPUT_BYTES,
    )?;
    const MAX_BYTES: usize = 256 * 1024;
    let truncated_bytes = bytes.len() > MAX_BYTES;
    let is_binary = bytes.iter().take(8192).any(|&b| b == 0);
    if is_binary {
        return Ok(json!({
            "path": path,
            "oid": commit_oid,
            "binary": true,
            "text": null,
            "truncated": truncated_bytes,
            "next_line": null,
        }));
    }
    let text = String::from_utf8_lossy(if truncated_bytes {
        &bytes[..MAX_BYTES]
    } else {
        &bytes
    });
    let lines: Vec<&str> = text.lines().collect();
    let start = (start_line.saturating_sub(1)) as usize;
    let end = (start + max_lines as usize).min(lines.len());
    let slice = if start < lines.len() {
        lines[start..end].join("\n")
    } else {
        String::new()
    };
    let next_line = if end < lines.len() {
        Some((end + 1) as u32)
    } else {
        None
    };
    Ok(json!({
        "path": path,
        "oid": commit_oid,
        "binary": false,
        "text": slice,
        "truncated": truncated_bytes || end < lines.len(),
        "next_line": next_line,
    }))
}

pub fn git_literal_search(
    bare: &Path,
    commit_oid: &str,
    query: &str,
    path_prefix: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    validate_git_oid(commit_oid).map_err(AppError::Validation)?;
    if query.is_empty() || query.len() > 256 {
        return Err(AppError::Validation("invalid query".to_string()));
    }
    let mut args = vec![
        "--git-dir".to_string(),
        bare.to_str()
            .ok_or_else(|| AppError::Other("git path".into()))?
            .to_string(),
        "grep".to_string(),
        "-n".to_string(),
        "--full-name".to_string(),
        query.to_string(),
        commit_oid.to_string(),
    ];
    if let Some(prefix) = path_prefix {
        validate_repo_path(prefix)?;
        args.push("--".to_string());
        args.push(format!("{prefix}*"));
    }
    let output = std::process::Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    for line in stdout.lines() {
        let Some((path, rest)) = line.split_once(':') else {
            continue;
        };
        let Some((line_no, snippet)) = rest.split_once(':') else {
            continue;
        };
        items.push(json!({
            "path": path,
            "line": line_no.parse::<i64>().unwrap_or(0),
            "snippet": snippet.chars().take(200).collect::<String>(),
            "commit_oid": commit_oid,
        }));
        if items.len() as i64 >= limit {
            break;
        }
    }
    let total = stdout.lines().filter(|l| !l.is_empty()).count();
    let item_count = items.len();
    Ok((items, total > item_count))
}

pub fn git_commit_compare(
    bare: &Path,
    base_oid: &str,
    head_oid: &str,
    path_filter: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, Value, bool), AppError> {
    validate_git_oid(base_oid).map_err(AppError::Validation)?;
    validate_git_oid(head_oid).map_err(AppError::Validation)?;
    let mut args = vec![
        "--git-dir".to_string(),
        bare.to_str()
            .ok_or_else(|| AppError::Other("git path".into()))?
            .to_string(),
        "diff".to_string(),
        "--name-status".to_string(),
        format!("{base_oid}..{head_oid}"),
    ];
    if let Some(p) = path_filter {
        validate_repo_path(p)?;
        args.push("--".to_string());
        args.push(p.to_string());
    }
    let output = std::process::Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(AppError::Validation("diff failed".to_string()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    let mut adds = 0i64;
    let mut dels = 0i64;
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let status = parts.next().unwrap_or("M");
        let old_path = parts.next().unwrap_or("");
        let new_path = parts.next().unwrap_or(old_path);
        let change_type = match status.chars().next() {
            Some('A') => "add",
            Some('D') => "delete",
            Some('M') => "modify",
            Some('R') => "rename",
            _ => "modify",
        };
        items.push(json!({
            "old_path": old_path,
            "new_path": new_path,
            "change_type": change_type,
            "binary": false,
            "additions": 0,
            "deletions": 0,
            "truncated": true,
        }));
        if change_type == "add" {
            adds += 1;
        }
        if change_type == "delete" {
            dels += 1;
        }
        if items.len() as i64 >= limit {
            break;
        }
    }
    let summary = json!({
        "files_changed": items.len(),
        "additions_hint": adds,
        "deletions_hint": dels,
    });
    let has_more = stdout.lines().filter(|l| !l.is_empty()).count() > items.len();
    Ok((items, summary, has_more))
}

pub fn git_pr_file_diff(
    bare: &Path,
    base_oid: &str,
    head_oid: &str,
    path: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    let (items, _, has_more) = git_commit_compare(bare, base_oid, head_oid, path, limit)?;
    Ok((items, has_more))
}

pub fn git_log_commits(
    bare: &Path,
    ref_oid: &str,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    validate_git_oid(ref_oid).map_err(AppError::Validation)?;
    let fetch_n = limit.saturating_add(1).max(1);
    let git_dir = bare
        .to_str()
        .ok_or_else(|| AppError::Other("git path".into()))?;
    let bytes = git_command_output_limited(
        &[
            "--git-dir",
            git_dir,
            "log",
            ref_oid,
            "--format=%H%x09%an%x09%s%x09%ct",
            "-n",
            &fetch_n.to_string(),
        ],
        MAX_GIT_CMD_OUTPUT_BYTES,
    )?;
    let stdout = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    let has_more = lines.len() as i64 > limit;
    let take = usize::try_from(limit).unwrap_or(50).min(lines.len());
    let mut items = Vec::new();
    for line in lines.iter().take(take) {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() < 4 {
            continue;
        }
        items.push(json!({
            "oid": parts[0],
            "author": parts[1],
            "summary": parts[2],
            "committed_at_epoch": parts[3].parse::<i64>().unwrap_or(0),
        }));
    }
    Ok((items, has_more))
}

pub fn git_unified_diff(
    bare: &Path,
    base_oid: &str,
    head_oid: &str,
    path: &str,
    max_bytes: usize,
) -> Result<Value, AppError> {
    validate_git_oid(base_oid).map_err(AppError::Validation)?;
    validate_git_oid(head_oid).map_err(AppError::Validation)?;
    validate_repo_path_non_empty(path)?;
    let git_dir = bare
        .to_str()
        .ok_or_else(|| AppError::Other("git path".into()))?;
    let spec = format!("{base_oid}..{head_oid}");
    let bytes = git_command_output_limited(
        &["--git-dir", git_dir, "diff", &spec, "--", path],
        MAX_GIT_CMD_OUTPUT_BYTES,
    )?;
    let truncated = bytes.len() > max_bytes;
    let slice = if truncated {
        &bytes[..max_bytes]
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(slice);
    let is_binary = bytes.iter().take(8192).any(|&b| b == 0);
    Ok(json!({
        "path": path,
        "base_oid": base_oid,
        "head_oid": head_oid,
        "binary": is_binary,
        "unified": if is_binary { Value::Null } else { json!(text) },
        "truncated": truncated,
    }))
}
