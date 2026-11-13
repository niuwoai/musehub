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
    let output = Command::new("git")
        .args(["init", "--bare", path.to_str().expect("utf8 path")])
        .output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if detail.is_empty() {
            "git init --bare failed".to_string()
        } else {
            format!("git init --bare failed: {detail}")
        };
        return Err(AppError::Other(msg));
    }
    seed_empty_main_branch(&path)?;
    let sym = Command::new("git")
        .args([
            "--git-dir",
            path.to_str().expect("utf8"),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !sym.success() {
        return Err(AppError::Other("set bare HEAD symref failed".to_string()));
    }
    crate::hooks::install_pre_receive_hook(&path, "main")?;
    #[cfg(unix)]
    apply_bare_repo_group_acl(&path)?;
    Ok(())
}

/// Lets the dedicated `git` transport user mutate receive-pack state (group shared with API writer).
#[cfg(unix)]
pub fn apply_bare_repo_group_acl(bare: &Path) -> Result<(), AppError> {
    crate::git_ssh_runtime::apply_bare_repo_group_acl_physical(bare)
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

fn storage_group_http_backend_command(
    git_root: &str,
    request_method: &str,
    path_info: &str,
    query: &str,
    content_type: &str,
    content_length: usize,
    remote_user: &str,
) -> Command {
    let content_length = content_length.to_string();
    let mut cmd = if let Some(group) = std::env::var("MUSEHUB_GIT_STORAGE_GROUP")
        .ok()
        .filter(|g| !g.is_empty())
    {
        let mut wrapped = Command::new("sg");
        wrapped.arg(group).arg("-c").arg("git http-backend");
        wrapped
    } else {
        let mut direct = Command::new("git");
        direct.arg("http-backend");
        direct
    };
    cmd.env("GIT_HTTP_EXPORT_ALL", "1")
        .env("GIT_PROJECT_ROOT", git_root)
        .env("REQUEST_METHOD", request_method)
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", content_type)
        .env("CONTENT_LENGTH", content_length)
        .env("REMOTE_USER", remote_user);
    cmd
}

#[allow(clippy::too_many_arguments)]
fn run_git_http_backend_blocking(
    method: &Method,
    storage_key: &uuid::Uuid,
    git_suffix: &str,
    query: &str,
    content_type: &str,
    body: &[u8],
    git_root: &Path,
    remote_user: &str,
) -> Result<Response, AppError> {
    let path_info = format!("/{}.git{}", storage_key, git_suffix);
    let git_root_s = git_root
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 git root".into()))?;
    let mut child = storage_group_http_backend_command(
        git_root_s,
        method.as_str(),
        &path_info,
        query,
        content_type,
        body.len(),
        remote_user,
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| AppError::Other(error.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(body)
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
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let method = method.clone();
    let storage_key = *storage_key;
    let git_suffix = git_suffix.to_string();
    let query = query.to_string();
    let body_vec = body.to_vec();
    let git_root = git_root.to_path_buf();
    let remote_user = remote_user.to_string();
    tokio::task::spawn_blocking(move || {
        run_git_http_backend_blocking(
            &method,
            &storage_key,
            &git_suffix,
            &query,
            &content_type,
            &body_vec,
            &git_root,
            &remote_user,
        )
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?
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

/// Base URL operators give to users for HTTP Git (`/git/...`). Prefer `MUSEHUB_PUBLIC_HTTP_ORIGIN`
/// (gateway front door); otherwise map container bind `0.0.0.0:port` to `http://127.0.0.1:port`.
pub fn public_http_origin(bind_display: &str) -> String {
    if let Ok(raw) = std::env::var("MUSEHUB_PUBLIC_HTTP_ORIGIN") {
        let trimmed = raw.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let host_port = if let Some(rest) = bind_display.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{rest}")
    } else if let Some(rest) = bind_display.strip_prefix("[::]:") {
        format!("127.0.0.1:{rest}")
    } else {
        bind_display.to_string()
    };
    if host_port.starts_with("http://") || host_port.starts_with("https://") {
        host_port.trim_end_matches('/').to_string()
    } else {
        format!("http://{host_port}")
    }
}

pub fn git_clone_url(bind_display: &str, org_slug: &str, repo_slug: &str) -> String {
    let origin = public_http_origin(bind_display);
    format!("{origin}/git/{org_slug}/{repo_slug}.git")
}

#[cfg(test)]
mod clone_url_tests {
    use super::*;

    #[test]
    fn public_origin_honors_env() {
        std::env::set_var("MUSEHUB_PUBLIC_HTTP_ORIGIN", "https://musehub.corp.example");
        assert_eq!(
            public_http_origin("0.0.0.0:8080"),
            "https://musehub.corp.example"
        );
        std::env::remove_var("MUSEHUB_PUBLIC_HTTP_ORIGIN");
    }

    #[test]
    fn clone_url_maps_wildcard_bind_to_loopback() {
        std::env::remove_var("MUSEHUB_PUBLIC_HTTP_ORIGIN");
        assert_eq!(
            git_clone_url("0.0.0.0:8080", "demo", "app"),
            "http://127.0.0.1:8080/git/demo/app.git"
        );
    }
}
