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
    fs::create_dir_all(run_dir).map_err(|e| AppError::Other(e.to_string()))?;
    #[cfg(unix)]
    {
        let mode = std::env::var("MUSEHUB_GIT_SSH_RUN_DIR_MODE")
            .ok()
            .and_then(|m| u32::from_str_radix(m.trim_start_matches('0'), 8).ok())
            .unwrap_or(0o700);
        restrict_dir_mode(run_dir, mode)?;
    }
    Ok(())
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
    if let Some(parent) = path.parent() {
        if parent.exists() {
            let meta = fs::symlink_metadata(parent).map_err(|e| AppError::Other(e.to_string()))?;
            if meta.file_type().is_symlink() {
                return Err(AppError::Validation(
                    "git ssh parent path must not be a symlink".into(),
                ));
            }
        }
    }
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
