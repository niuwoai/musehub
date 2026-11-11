//! Product Git-over-SSH gateway (OpenSSH AuthorizedKeysCommand + forced command).

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use sqlx::PgPool;
use uuid::Uuid;

use crate::audit;
use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::grants;

pub const ALLOWED_GIT_COMMANDS: [&str; 2] = ["git-upload-pack", "git-receive-pack"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSshService {
    UploadPack,
    ReceivePack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGitSshCommand {
    pub service: GitSshService,
    pub org: String,
    pub repo: String,
}

/// Parses OpenSSH `SSH_ORIGINAL_COMMAND` for MuseHub logical paths `org/repo[.git]`.
pub fn parse_ssh_original_command(raw: &str) -> Result<ParsedGitSshCommand, AppError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AppError::Validation("empty ssh command".into()));
    }
    let (cmd, path) = if let Some(stripped) = raw.strip_prefix("git-upload-pack ") {
        (GitSshService::UploadPack, stripped)
    } else if let Some(stripped) = raw.strip_prefix("git-receive-pack ") {
        (GitSshService::ReceivePack, stripped)
    } else {
        return Err(AppError::Validation("ssh command not allowed".into()));
    };
    let path = path.trim().trim_matches('\'').trim_matches('"');
    if path.is_empty() || path.contains("..") || path.contains('\0') {
        return Err(AppError::Validation("invalid repository path".into()));
    }
    let path = path.trim_start_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let Some((org, repo)) = path.split_once('/') else {
        return Err(AppError::Validation(
            "repository path must be org/repo".into(),
        ));
    };
    for segment in [org, repo] {
        if segment.is_empty() || segment.len() > 64 {
            return Err(AppError::Validation("invalid path segment".into()));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(AppError::Validation("invalid path segment".into()));
        }
    }
    Ok(ParsedGitSshCommand {
        service: cmd,
        org: org.to_string(),
        repo: repo.to_string(),
    })
}

/// SSH host for clone URLs (`MUSEHUB_PUBLIC_SSH_HOST` or loopback).
pub fn public_ssh_host(bind_display: &str) -> String {
    if let Ok(raw) = std::env::var("MUSEHUB_PUBLIC_SSH_HOST") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(port) = std::env::var("MUSEHUB_PUBLIC_SSH_PORT") {
        let _ = port;
    }
    if bind_display.starts_with("0.0.0.0:") || bind_display.starts_with("[::]:") {
        "127.0.0.1".to_string()
    } else {
        bind_display
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string()
    }
}

pub fn git_ssh_port() -> u16 {
    std::env::var("MUSEHUB_GIT_SSH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2222)
}

pub fn git_ssh_clone_url(bind_display: &str, org_slug: &str, repo_slug: &str) -> String {
    let host = public_ssh_host(bind_display);
    let port = git_ssh_port();
    if port == 22 {
        format!("git@{host}:{org_slug}/{repo_slug}.git")
    } else {
        format!("ssh://git@{host}:{port}/{org_slug}/{repo_slug}.git")
    }
}

#[derive(Debug, Clone)]
pub struct SshKeyAuth {
    pub key_id: Uuid,
    pub principal_id: Uuid,
    pub handle: String,
    pub public_key: String,
}

pub async fn lookup_ssh_key_by_fingerprint(
    pool: &PgPool,
    fingerprint: &str,
) -> Result<SshKeyAuth, AppError> {
    let fp = fingerprint.trim();
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, String)>(
        "SELECT s.id, s.principal_id, p.handle, p.status, s.public_key
         FROM ssh_keys s
         JOIN principals p ON p.id = s.principal_id
         WHERE s.fingerprint = $1 AND s.revoked_at IS NULL",
    )
    .bind(fp)
    .fetch_optional(pool)
    .await?;
    let Some((key_id, principal_id, handle, status, public_key)) = row else {
        return Err(AppError::Unauthorized);
    };
    if status != "active" {
        return Err(AppError::Unauthorized);
    }
    let member = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM memberships WHERE principal_id = $1 AND status = 'active'",
    )
    .bind(principal_id)
    .fetch_one(pool)
    .await?;
    if member.0 == 0 {
        return Err(AppError::Unauthorized);
    }
    Ok(SshKeyAuth {
        key_id,
        principal_id,
        handle,
        public_key: public_key.trim().to_string(),
    })
}

pub async fn reload_ssh_key_session(
    pool: &PgPool,
    key_id: Uuid,
    principal_id: Uuid,
) -> Result<SshKeyAuth, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String)>(
        "SELECT s.principal_id, p.handle, p.status, s.public_key
         FROM ssh_keys s
         JOIN principals p ON p.id = s.principal_id
         WHERE s.id = $1 AND s.principal_id = $2 AND s.revoked_at IS NULL",
    )
    .bind(key_id)
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    let Some((pid, handle, status, public_key)) = row else {
        return Err(AppError::Unauthorized);
    };
    if status != "active" || pid != principal_id {
        return Err(AppError::Unauthorized);
    }
    Ok(SshKeyAuth {
        key_id,
        principal_id,
        handle,
        public_key: public_key.trim().to_string(),
    })
}

/// Writes one OpenSSH `authorized_keys` line for sshd `AuthorizedKeysCommand`.
pub fn format_authorized_keys_line(auth: &SshKeyAuth, gateway_bin: &str) -> String {
    format!(
        "environment=\"MUSEHUB_SSH_KEY_ID={}\",environment=\"MUSEHUB_SSH_PRINCIPAL_ID={}\",no-agent-forwarding,no-port-forwarding,no-pty,no-user-rc,no-X11-forwarding,command=\"{gateway_bin} gateway\" {}",
        auth.key_id,
        auth.principal_id,
        auth.public_key.trim()
    )
}

pub async fn run_ssh_gateway(
    pool: &PgPool,
    git_root: &Path,
    key_id: Uuid,
    principal_id: Uuid,
    original_command: &str,
) -> Result<i32, AppError> {
    let _auth = reload_ssh_key_session(pool, key_id, principal_id).await?;
    let parsed = parse_ssh_original_command(original_command)?;
    let (_, repo_id, storage_key) =
        super::repo_storage_for_member(pool, &parsed.org, &parsed.repo, principal_id).await?;
    let org_id = super::org_id_for_member(pool, &parsed.org, principal_id).await?;
    grants::ensure_principal_repo_access(pool, org_id, repo_id, principal_id).await?;
    let bare = bare_repo_path(git_root, &storage_key);
    if !bare.is_dir() {
        return Err(AppError::NotFound);
    }
    let service_name = match parsed.service {
        GitSshService::UploadPack => "upload-pack",
        GitSshService::ReceivePack => "receive-pack",
    };
    let metadata = serde_json::json!({
        "transport": "ssh",
        "service": service_name,
        "org": parsed.org,
        "repo": parsed.repo,
    });
    let _ = audit::record(
        pool,
        Some(org_id),
        Some(repo_id),
        principal_id,
        Some(principal_id),
        &format!("git.{service_name}"),
        "repository",
        repo_id,
        None,
        "allow",
        metadata,
    )
    .await;
    run_git_pack_inherit(&bare, service_name)
}

fn run_git_pack_inherit(bare: &Path, service: &str) -> Result<i32, AppError> {
    let mut child = Command::new("git");
    child.arg(service).arg(bare);
    child.stdin(Stdio::inherit());
    child.stdout(Stdio::inherit());
    child.stderr(Stdio::inherit());
    let status = child.status().map_err(|e| AppError::Other(e.to_string()))?;
    Ok(status.code().unwrap_or(1))
}

/// Render sshd_config fragment for product Git SSH (caller supplies absolute paths).
pub fn render_sshd_config(
    port: u16,
    host_key_path: &Path,
    pid_file: &Path,
    authorized_keys_bin: &Path,
) -> String {
    format!(
        r#"Port {port}
ListenAddress 127.0.0.1
HostKey {host_key}
PidFile {pid}
AuthorizedKeysFile none
AuthorizedKeysCommand {akc} authorized-keys %u %k %t %f
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PubkeyAuthentication yes
PermitTTY no
AllowTcpForwarding no
X11Forwarding no
GatewayPorts no
AllowAgentForwarding no
PermitUserRC no
UsePAM no
LogLevel ERROR
Subsystem sftp /bin/false
"#,
        port = port,
        host_key = host_key_path.display(),
        pid = pid_file.display(),
        akc = authorized_keys_bin.display(),
    )
}

pub fn write_authorized_keys_stdout(auth: &SshKeyAuth, gateway_bin: &str) -> io::Result<()> {
    let line = format_authorized_keys_line(auth, gateway_bin);
    let mut out = io::stdout().lock();
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_upload_pack_logical_path() {
        let p = parse_ssh_original_command("git-upload-pack 'demo/app.git'").unwrap();
        assert_eq!(p.service, GitSshService::UploadPack);
        assert_eq!(p.org, "demo");
        assert_eq!(p.repo, "app");
    }

    #[test]
    fn parse_rejects_shell() {
        assert!(parse_ssh_original_command("sh").is_err());
        assert!(parse_ssh_original_command("git-upload-pack '../etc/passwd'").is_err());
    }
}
