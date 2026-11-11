//! Secure host key / run directory preparation for product Git SSH.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::AppError;

pub fn default_git_ssh_run_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("MUSEHUB_GIT_SSH_RUN_DIR")
            .unwrap_or_else(|_| "/var/lib/musehub/git-ssh".into()),
    )
}

pub fn prepare_git_ssh_run_dir(run_dir: &Path) -> Result<(), AppError> {
    if run_dir.as_os_str().is_empty() {
        return Err(AppError::Validation("invalid git ssh run dir".into()));
    }
    reject_symlink_path(run_dir)?;
    #[cfg(unix)]
    validate_secure_ancestor_chain(run_dir)?;
    fs::create_dir_all(run_dir).map_err(|e| AppError::Other(e.to_string()))?;
    #[cfg(unix)]
    {
        let mode = std::env::var("MUSEHUB_GIT_SSH_RUN_DIR_MODE")
            .ok()
            .and_then(|m| u32::from_str_radix(m.trim_start_matches('0'), 8).ok())
            .unwrap_or(0o700);
        restrict_dir_mode(run_dir, mode)?;
        validate_secure_ancestor_chain(run_dir)?;
    }
    Ok(())
}

pub fn git_ssh_runtime_env_path(run_dir: &Path) -> PathBuf {
    run_dir.join("runtime.env")
}

fn shell_quote_sh(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Credentials and gateway paths — mode 0640 root:git, not embedded in world-readable scripts.
pub fn write_git_ssh_runtime_env(
    run_dir: &Path,
    vars: &[(&str, &str)],
) -> Result<PathBuf, AppError> {
    let path = git_ssh_runtime_env_path(run_dir);
    let body = vars
        .iter()
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .map(|(k, v)| format!("{k}={}", shell_quote_sh(v)))
        .collect::<Vec<_>>()
        .join("\n");
    write_secure_file(&path, body.as_bytes()).map_err(|e| AppError::Other(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640))
            .map_err(|e| AppError::Other(e.to_string()))?;
        let group = std::env::var("MUSEHUB_GIT_SSH_RUNTIME_GROUP").unwrap_or_else(|_| "git".into());
        let p = path
            .to_str()
            .ok_or_else(|| AppError::Other("utf8".into()))?;
        let _ = Command::new("chown")
            .args([&format!("root:{group}"), p])
            .status();
        let _ = Command::new("sudo")
            .args(["-n", "chown", &format!("root:{group}"), p])
            .status();
    }
    Ok(path)
}

pub fn ensure_host_ed25519_key(host_key: &Path) -> Result<(), AppError> {
    reject_symlink_path(host_key)?;
    if host_key.exists() {
        validate_existing_host_key(host_key)?;
        return Ok(());
    }
    if let Some(parent) = host_key.parent() {
        prepare_git_ssh_run_dir(parent)?;
    }
    let status = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            host_key
                .to_str()
                .ok_or_else(|| AppError::Other("path".into()))?,
            "-N",
            "",
            "-q",
        ])
        .status()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !status.success() {
        return Err(AppError::Other("ssh-keygen host key failed".into()));
    }
    #[cfg(unix)]
    restrict_file_mode(host_key, 0o600)?;
    Ok(())
}

fn reject_symlink_path(path: &Path) -> Result<(), AppError> {
    if path.exists() {
        let meta = fs::symlink_metadata(path).map_err(|e| AppError::Other(e.to_string()))?;
        if meta.file_type().is_symlink() {
            return Err(AppError::Validation(
                "git ssh path must not be a symlink".into(),
            ));
        }
    }
    let mut cur = path.parent();
    while let Some(parent) = cur {
        if parent.as_os_str().is_empty() {
            break;
        }
        if parent.exists() {
            let meta = fs::symlink_metadata(parent).map_err(|e| AppError::Other(e.to_string()))?;
            if meta.file_type().is_symlink() {
                return Err(AppError::Validation(
                    "git ssh ancestor path must not be a symlink".into(),
                ));
            }
        }
        if parent.parent().is_none() || parent == Path::new("/") {
            break;
        }
        cur = parent.parent();
    }
    Ok(())
}

#[cfg(unix)]
fn validate_secure_ancestor_chain(path: &Path) -> Result<(), AppError> {
    let enforce = std::env::var("MUSEHUB_GIT_SSH_VALIDATE_ANCESTORS")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or_else(|| {
            path.starts_with("/var/lib/musehub") || path.starts_with("/var/lib/musehub-ssh-test-")
        });
    if !enforce {
        return Ok(());
    }
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mut cur = path;
    while let Some(parent) = cur.parent() {
        if parent.as_os_str().is_empty() {
            break;
        }
        if parent.exists() {
            let meta = fs::metadata(parent).map_err(|e| AppError::Other(e.to_string()))?;
            if !meta.is_dir() {
                return Err(AppError::Validation(
                    "git ssh ancestor must be a directory".into(),
                ));
            }
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o002 != 0 {
                return Err(AppError::Validation(
                    "git ssh ancestor directory must not be world-writable".into(),
                ));
            }
            if meta.uid() != 0 {
                return Err(AppError::Validation(
                    "git ssh ancestor directory must be root-owned".into(),
                ));
            }
        }
        if parent == Path::new("/") {
            break;
        }
        cur = parent;
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_secure_ancestor_chain(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

fn validate_existing_host_key(path: &Path) -> Result<(), AppError> {
    let meta = fs::metadata(path).map_err(|e| AppError::Other(e.to_string()))?;
    if !meta.is_file() {
        return Err(AppError::Validation(
            "existing host key path is not a regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(AppError::Validation(
                "existing host key has unsafe permissions".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_dir_mode(path: &Path, mode: u32) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| AppError::Other(e.to_string()))
}

#[cfg(unix)]
fn restrict_file_mode(path: &Path, mode: u32) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| AppError::Other(e.to_string()))
}

#[cfg(not(unix))]
fn restrict_dir_mode(_path: &Path, _mode: u32) -> Result<(), AppError> {
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file_mode(_path: &Path, _mode: u32) -> Result<(), AppError> {
    Ok(())
}

pub fn write_secure_file(path: &Path, body: &[u8]) -> Result<(), io::Error> {
    fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
