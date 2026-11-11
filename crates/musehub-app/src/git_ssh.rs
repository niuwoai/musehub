//! Product Git-over-SSH gateway (OpenSSH AuthorizedKeysCommand + forced command).
//!
//! **Session identity:** Gateway loads the SSH key from `MUSEHUB_SSH_KEY_FP` set on the
//! AuthorizedKeysCommand line for the pubkey that authenticated. That is trustworthy only when
//! `serve` has pinned the `git` **passwd** home (`pw_dir`) to a root-owned non-writable directory
//! with no `~/.ssh/environment`, because OpenSSH reads `~/.ssh/environment` from passwd home, not
//! session `HOME`. Client `SendEnv` / writable passwd home can otherwise override allowlisted vars.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use sqlx::PgPool;
use uuid::Uuid;

use crate::audit;
use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::grants;
use crate::ssh_keys::sanitize_stored_ssh_public_key;

/// Unix account clients must use (`ssh git@host`, `AllowUsers`).
pub const GIT_SSH_LOGIN_USER: &str = "git";

pub fn assert_git_ssh_connection_user(ssh_user: &str) -> Result<(), AppError> {
    if ssh_user.trim() == GIT_SSH_LOGIN_USER {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

/// AKC-bound fingerprint for this session (`environment="MUSEHUB_SSH_KEY_FP=…"`).
///
/// Requires `serve`-time `verify_git_ssh_passwd_home` (locked passwd `pw_dir`).
pub fn session_akc_bound_pubkey_fingerprint() -> Result<String, AppError> {
    std::env::var("MUSEHUB_SSH_KEY_FP").map_err(|_| AppError::Unauthorized)
}

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
fn extract_forced_git_command(inner: &str, cmd: &str) -> Option<String> {
    let needle = format!("{cmd} ");
    let start = inner.find(&needle)?;
    let rest = &inner[start..];
    let open = rest.find('\'')?;
    let path = rest.get(open + 1..)?;
    let close = path.find('\'')?;
    Some(format!("{cmd} '{}'", &path[..close]))
}

/// OpenSSH often passes `-c 'git-upload-pack …'` as the original command for custom login shells.
pub fn normalize_ssh_original_command(raw: &str) -> String {
    let raw = raw.trim();
    let inner = raw.strip_prefix("-c ").unwrap_or(raw).trim();
    for cmd in ALLOWED_GIT_COMMANDS {
        if let Some(extracted) = extract_forced_git_command(inner, cmd) {
            return extracted;
        }
    }
    inner.trim_matches('\'').to_string()
}

pub fn parse_ssh_original_command(raw: &str) -> Result<ParsedGitSshCommand, AppError> {
    let raw = normalize_ssh_original_command(raw).trim().to_string();
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
    let public_key = sanitize_stored_ssh_public_key(&public_key)?;
    Ok(SshKeyAuth {
        key_id,
        principal_id,
        handle,
        public_key,
    })
}

/// Session auth uses only `key_id` from sshd-forced env; principal comes from DB (not client-settable).
pub async fn reload_ssh_key_by_id(pool: &PgPool, key_id: Uuid) -> Result<SshKeyAuth, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String)>(
        "SELECT s.principal_id, p.handle, p.status, s.public_key
         FROM ssh_keys s
         JOIN principals p ON p.id = s.principal_id
         WHERE s.id = $1 AND s.revoked_at IS NULL",
    )
    .bind(key_id)
    .fetch_optional(pool)
    .await?;
    let Some((principal_id, handle, status, public_key)) = row else {
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
    let public_key = sanitize_stored_ssh_public_key(&public_key)?;
    Ok(SshKeyAuth {
        key_id,
        principal_id,
        handle,
        public_key,
    })
}

/// Writes one OpenSSH `authorized_keys` line for sshd `AuthorizedKeysCommand`.
pub fn format_authorized_keys_line(
    auth: &SshKeyAuth,
    gateway_bin: &str,
    fingerprint: &str,
) -> Result<String, AppError> {
    let key_material = sanitize_stored_ssh_public_key(&auth.public_key)?;
    if key_material.contains('\n') || key_material.contains('\r') {
        return Err(AppError::Validation("invalid stored ssh public key".into()));
    }
    let fp = fingerprint.trim();
    if fp.is_empty() {
        return Err(AppError::Validation("ssh fingerprint required".into()));
    }
    let gateway_bin = gateway_bin.replace('"', "");
    let _gateway_bin = gateway_bin;
    Ok(format!(
        r#"environment="MUSEHUB_SSH_KEY_ID={key_id}",environment="MUSEHUB_SSH_KEY_FP={fp}",no-agent-forwarding,no-port-forwarding,no-pty,no-user-rc,no-X11-forwarding {key_material}"#,
        key_id = auth.key_id,
        fp = fp.replace('"', ""),
    ))
}

pub async fn run_ssh_gateway(
    pool: &PgPool,
    git_root: &Path,
    key_id: Uuid,
    original_command: &str,
) -> Result<i32, AppError> {
    let auth = reload_ssh_key_by_id(pool, key_id).await?;
    let principal_id = auth.principal_id;
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
    child
        .arg("-c")
        .arg(format!("safe.directory={}", bare.display()));
    child.arg(service).arg(bare);
    child.stdin(Stdio::inherit());
    child.stdout(Stdio::inherit());
    child.stderr(Stdio::inherit());
    let status = child.status().map_err(|e| AppError::Other(e.to_string()))?;
    Ok(status.code().unwrap_or(1))
}

/// Render sshd_config fragment for product Git SSH (caller supplies absolute paths).
pub fn ssh_listen_address() -> String {
    std::env::var("MUSEHUB_GIT_SSH_LISTEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn render_sshd_config(
    port: u16,
    host_key_path: &Path,
    pid_file: &Path,
    authorized_keys_bin: &Path,
    setenv: &[(&str, &str)],
    login_user: &str,
    akc_user: &str,
    akc_is_wrapper: bool,
) -> String {
    let log_level = std::env::var("MUSEHUB_GIT_SSH_SSHD_LOG")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ERROR".to_string());
    let listen = ssh_listen_address();
    let akc_cmd = if akc_is_wrapper {
        format!("{}", authorized_keys_bin.display())
    } else {
        format!("{} authorized-keys", authorized_keys_bin.display())
    };
    let mut env_lines = String::new();
    for (k, v) in setenv {
        if k.is_empty() || v.is_empty() {
            continue;
        }
        let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
        env_lines.push_str(&format!("SetEnv {k}=\"{escaped}\"\n"));
    }
    format!(
        r#"Port {port}
ListenAddress {listen}
HostKey {host_key}
PidFile {pid}
{env_lines}PermitRootLogin no
PermitUserEnvironment MUSEHUB_SSH_KEY_ID,MUSEHUB_SSH_KEY_FP
AllowUsers {login_user}
AuthorizedKeysFile none
AuthorizedKeysCommand {akc_cmd} %u %k %t %f
AuthorizedKeysCommandUser {akc_user}
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
LogLevel {log_level}
Subsystem sftp /bin/false
"#,
        port = port,
        listen = listen,
        host_key = host_key_path.display(),
        pid = pid_file.display(),
        akc_cmd = akc_cmd,
        login_user = login_user,
        akc_user = akc_user,
        log_level = log_level,
    )
}

pub fn write_authorized_keys_stdout(
    auth: &SshKeyAuth,
    gateway_bin: &str,
    fingerprint: &str,
) -> io::Result<()> {
    let line = format_authorized_keys_line(auth, gateway_bin, fingerprint)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let mut out = io::stdout().lock();
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_openssh_c_wrapper() {
        let n = normalize_ssh_original_command("-c 'git-upload-pack 'demo/app.git''");
        assert_eq!(n, "git-upload-pack 'demo/app.git'");
    }

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

    #[test]
    fn sshd_config_allows_only_key_binding_env() {
        let body = render_sshd_config(
            2222,
            Path::new("/var/lib/musehub/git-ssh/ssh_host_ed25519_key"),
            Path::new("/var/lib/musehub/git-ssh/sshd.pid"),
            Path::new("/usr/local/bin/musehub-git-ssh"),
            &[(
                "MUSEHUB_GIT_SSH_RUNTIME_ENV",
                "/var/lib/musehub/git-ssh/runtime.env",
            )],
            GIT_SSH_LOGIN_USER,
            "git",
            false,
        );
        assert!(body.contains("PermitUserEnvironment MUSEHUB_SSH_KEY_ID,MUSEHUB_SSH_KEY_FP"));
        assert!(!body.contains("ExposeAuthInfo"));
        assert!(!body.contains("PermitUserEnvironment yes"));
        assert!(!body.contains("restrict"));
    }

    #[test]
    fn authorized_keys_line_is_key_bound_not_principal_spoofable() {
        let auth = SshKeyAuth {
            key_id: Uuid::new_v4(),
            principal_id: Uuid::new_v4(),
            handle: "u".into(),
            public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICXd1mIaYHo4ZxmswgaeWTij03jgeXse1S6HqIvQ1jrs test@musehub".into(),
        };
        let line = format_authorized_keys_line(&auth, "/opt/bin/musehub-git-ssh", "SHA256:testfp")
            .unwrap();
        assert!(line.contains(&format!("environment=\"MUSEHUB_SSH_KEY_ID={}", auth.key_id)));
        assert!(line.contains("MUSEHUB_SSH_KEY_FP=SHA256:testfp"));
        assert!(!line.contains("MUSEHUB_SSH_PRINCIPAL_ID"));
        assert!(!line.contains("command="));
        assert_eq!(line.matches('\n').count(), 0);
    }
}
