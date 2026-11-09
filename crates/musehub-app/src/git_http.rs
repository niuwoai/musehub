use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::Response;
use bytes::Bytes;

use crate::error::AppError;

pub fn bare_repo_path(git_root: &Path, storage_key: &uuid::Uuid) -> PathBuf {
    git_root.join(format!("{storage_key}.git"))
}

pub fn init_bare_repository(git_root: &Path, storage_key: &uuid::Uuid) -> Result<(), AppError> {
    let path = bare_repo_path(git_root, storage_key);
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(git_root).map_err(|error| AppError::Other(error.to_string()))?;
    let status = Command::new("git")
        .args(["init", "--bare", path.to_str().expect("utf8 path")])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !status.success() {
        return Err(AppError::Other("git init --bare failed".to_string()));
    }
    seed_empty_main_branch(&path)?;
    Ok(())
}

fn seed_empty_main_branch(bare: &Path) -> Result<(), AppError> {
    let temp = tempfile::tempdir().map_err(|error| AppError::Other(error.to_string()))?;
    let work = temp.path();
    let init = Command::new("git")
        .args(["init", work.to_str().expect("utf8")])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !init.success() {
        return Err(AppError::Other("git init worktree failed".to_string()));
    }
    let commit = Command::new("git")
        .current_dir(work)
        .args([
            "-c",
            "user.email=git@musehub.local",
            "-c",
            "user.name=musehub",
            "commit",
            "--allow-empty",
            "-m",
            "init",
            "--no-gpg-sign",
        ])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !commit.success() {
        return Err(AppError::Other("initial commit failed".to_string()));
    }
    let push = Command::new("git")
        .current_dir(work)
        .args(["push", bare.to_str().expect("utf8"), "HEAD:main"])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !push.success() {
        return Err(AppError::Other("push initial main failed".to_string()));
    }
    Ok(())
}

/// Proxies a Git smart-HTTP request to `git http-backend`.
#[allow(clippy::too_many_arguments)]
pub async fn git_http_backend(
    method: &Method,
    storage_key: &uuid::Uuid,
    git_suffix: &str,
    query: &str,
    headers: &HeaderMap,
    body: Bytes,
    git_root: &Path,
    remote_user: &str,
) -> Result<Response, AppError> {
    let path_info = format!("/{}.git{}", storage_key, git_suffix);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut child = Command::new("git")
        .arg("http-backend")
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("GIT_PROJECT_ROOT", git_root)
        .env("REQUEST_METHOD", method.as_str())
        .env("PATH_INFO", &path_info)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", content_type)
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("REMOTE_USER", remote_user)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(&body)
            .map_err(|error| AppError::Other(error.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !output.stderr.is_empty() {
        tracing::debug!(
            target: "musehub.git",
            stderr = %String::from_utf8_lossy(&output.stderr),
            "git http-backend"
        );
    }
    let (status, response_headers, body_start) = parse_cgi_output(&output.stdout)?;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = Response::builder().status(status);
    for (name, value) in response_headers {
        response = response.header(name, value);
    }
    let body = Body::from(body_start);
    Ok(response.body(body).unwrap())
}

fn parse_cgi_output(stdout: &[u8]) -> Result<(u16, Vec<(String, String)>, Bytes), AppError> {
    let split = stdout
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .or_else(|| {
            stdout
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| index + 2)
        })
        .unwrap_or(stdout.len());
    let header_block = String::from_utf8_lossy(&stdout[..split.min(stdout.len())]);
    let mut status = 200u16;
    let mut headers = Vec::new();
    for line in header_block.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Status:") {
            if let Some(code) = rest.split_whitespace().next() {
                status = code.parse().unwrap_or(500);
            }
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let body = Bytes::copy_from_slice(&stdout[split.min(stdout.len())..]);
    if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        return Ok((status, headers, body));
    }
    headers.push((
        "Content-Type".to_string(),
        "application/octet-stream".to_string(),
    ));
    Ok((status, headers, body))
}

pub fn git_clone_url(bind_display: &str, org_slug: &str, repo_slug: &str) -> String {
    format!("http://{bind_display}/git/{org_slug}/{repo_slug}.git")
}
