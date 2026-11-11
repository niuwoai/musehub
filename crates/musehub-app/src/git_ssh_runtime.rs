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

pub fn default_git_ssh_run_dir_mode() -> u32 {
    std::env::var("MUSEHUB_GIT_SSH_RUN_DIR_MODE")
        .ok()
        .and_then(|m| u32::from_str_radix(m.trim_start_matches('0'), 8).ok())
        .unwrap_or(0o750)
}

pub fn git_ssh_runtime_group() -> String {
    std::env::var("MUSEHUB_GIT_SSH_RUNTIME_GROUP").unwrap_or_else(|_| "git".into())
}

pub fn git_storage_group() -> String {
    std::env::var("MUSEHUB_GIT_STORAGE_GROUP").unwrap_or_else(|_| "musehub-git".into())
}

/// Re-apply production ACL after writing keys/scripts into the run dir.
#[cfg(unix)]
pub fn finalize_git_ssh_run_dir(run_dir: &Path) -> Result<(), AppError> {
    set_run_dir_root_git_group(run_dir)?;
    validate_run_dir_git_traverse(run_dir)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn finalize_git_ssh_run_dir(_run_dir: &Path) -> Result<(), AppError> {
    Ok(())
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
        let mode = default_git_ssh_run_dir_mode();
        restrict_dir_mode(run_dir, mode)?;
        set_run_dir_root_git_group(run_dir)?;
        validate_secure_ancestor_chain(run_dir)?;
        validate_run_dir_git_traverse(run_dir)?;
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
        chown_root_group(&path, &git_ssh_runtime_group())?;
        let meta = fs::metadata(&path).map_err(|e| AppError::Other(e.to_string()))?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o007 != 0 {
            return Err(AppError::Validation(
                "runtime.env must not be world-accessible".into(),
            ));
        }
        if mode & 0o040 == 0 {
            return Err(AppError::Validation(
                "runtime.env must be group-readable for git transport".into(),
            ));
        }
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

#[cfg(unix)]
pub fn is_root_euid() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
pub fn is_root_euid() -> bool {
    false
}

/// Run a host command as root (direct when euid 0, else passwordless sudo).
#[cfg(unix)]
pub fn run_privileged(command: &str, args: &[&str]) -> Result<(), AppError> {
    let run = |prog: &str, a: &[&str]| {
        Command::new(prog)
            .args(a)
            .status()
            .map_err(|e| AppError::Other(e.to_string()))
    };
    if run(command, args)?.success() {
        return Ok(());
    }
    if is_root_euid() {
        return Err(AppError::Other(format!("{command} failed")));
    }
    let status = Command::new("sudo")
        .arg("-n")
        .arg(command)
        .args(args)
        .status()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !status.success() {
        return Err(AppError::Other(format!("sudo {command} failed")));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn run_privileged(_command: &str, _args: &[&str]) -> Result<(), AppError> {
    Ok(())
}

/// Run `sh -c` as an unprivileged login user (su when root, else sudo -u).
#[cfg(unix)]
fn run_sh_as_user(user: &str, shell_cmd: &str) -> Result<bool, AppError> {
    let ok = if is_root_euid() {
        Command::new("runuser")
            .args(["-u", user, "--", "sh", "-c", shell_cmd])
            .status()
            .map_err(|e| AppError::Other(e.to_string()))?
            .success()
    } else {
        Command::new("sudo")
            .args(["-u", user, "sh", "-c", shell_cmd])
            .status()
            .map_err(|e| AppError::Other(e.to_string()))?
            .success()
    };
    Ok(ok)
}

#[cfg(unix)]
pub fn chown_root_group(path: &Path, group: &str) -> Result<(), AppError> {
    let p = path
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 path".into()))?;
    let spec = format!("root:{group}");
    let direct = Command::new("chown")
        .args([&spec, p])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let elevated = if direct {
        true
    } else {
        Command::new("sudo")
            .args(["-n", "chown", &spec, p])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !elevated {
        return Err(AppError::Other(format!(
            "chown {spec} failed for {}",
            path.display()
        )));
    }
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(|e| AppError::Other(e.to_string()))?;
    if meta.uid() != 0 {
        return Err(AppError::Validation(format!(
            "{} must be root-owned",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn set_run_dir_root_git_group(run_dir: &Path) -> Result<(), AppError> {
    chown_root_group(run_dir, &git_ssh_runtime_group())?;
    let mode = default_git_ssh_run_dir_mode();
    restrict_dir_mode(run_dir, mode)?;
    let p = run_dir
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 path".into()))?;
    let mode_s = format!("{mode:o}");
    let chmod_ok = Command::new("chmod")
        .args([&mode_s, p])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || Command::new("sudo")
            .args(["-n", "chmod", &mode_s, p])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if !chmod_ok {
        return Err(AppError::Other(format!("chmod {mode_s} on run dir failed")));
    }
    let meta = fs::metadata(run_dir).map_err(|e| AppError::Other(e.to_string()))?;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if meta.uid() != 0 {
        return Err(AppError::Validation(
            "git ssh run dir must be root-owned".into(),
        ));
    }
    let perms = meta.permissions().mode() & 0o777;
    if perms & 0o007 != 0 {
        return Err(AppError::Validation(format!(
            "git ssh run dir must not be world-accessible (mode={perms:o})"
        )));
    }
    if perms & 0o002 != 0 || perms & 0o020 != 0 {
        return Err(AppError::Validation(
            "git ssh run dir must not be group- or world-writable".into(),
        ));
    }
    if perms & 0o050 == 0 {
        return Err(AppError::Validation(
            "git ssh run dir must grant group traverse (e.g. 0750)".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub fn validate_git_ssh_runtime_readable(run_dir: &Path) -> Result<(), AppError> {
    let runtime = git_ssh_runtime_env_path(run_dir);
    if !runtime.exists() {
        return Err(AppError::Validation(
            "runtime.env required before validating git ssh readability".into(),
        ));
    }
    let p = runtime
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 path".into()))?;
    let git_user = git_ssh_runtime_group();
    let run_dir_s = run_dir.to_str().unwrap_or("");
    let traverse = run_sh_as_user(&git_user, &format!("test -x {run_dir_s}"))?;
    if !traverse {
        return Err(AppError::Validation(
            "git user must traverse git ssh run dir".into(),
        ));
    }
    let readable = run_sh_as_user(&git_user, &format!("test -r {p}"))?;
    if !readable {
        return Err(AppError::Validation(
            "git user must read runtime.env under production run dir modes".into(),
        ));
    }
    let writable = run_sh_as_user(&git_user, &format!("test -w {p}"))?;
    if writable {
        return Err(AppError::Validation(
            "git user must not write runtime.env".into(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn validate_git_ssh_runtime_readable(_run_dir: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
fn validate_run_dir_git_traverse(run_dir: &Path) -> Result<(), AppError> {
    let runtime = git_ssh_runtime_env_path(run_dir);
    if runtime.exists() {
        validate_git_ssh_runtime_readable(run_dir)?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn storage_sudo_fallback_enabled() -> bool {
    std::env::var("MUSEHUB_GIT_STORAGE_SUDO_FALLBACK")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
}

#[cfg(unix)]
fn resolve_group_gid(group: &str) -> Result<u32, AppError> {
    let out = Command::new("getent")
        .args(["group", group])
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !out.status.success() {
        return Err(AppError::Other(format!("getent group {group} failed")));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let gid = line
        .trim()
        .split(':')
        .nth(2)
        .ok_or_else(|| AppError::Other("gid field missing in getent".into()))?;
    gid.parse()
        .map_err(|e: std::num::ParseIntError| AppError::Other(e.to_string()))
}

#[cfg(unix)]
fn run_storage_chmod(mode: &str, path: &str) -> Result<(), AppError> {
    let ok = Command::new("chmod")
        .args([mode, path])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    if storage_sudo_fallback_enabled() {
        return run_privileged("chmod", &[mode, path]);
    }
    Err(AppError::Other(format!("chmod {mode} on {path} failed")))
}

#[cfg(unix)]
pub fn preflight_bare_repo_tree(bare: &Path) -> Result<(), AppError> {
    reject_symlink_path(bare)?;
    let mut stack = vec![bare.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = fs::symlink_metadata(&dir).map_err(|e| AppError::Other(e.to_string()))?;
        if meta.file_type().is_symlink() {
            return Err(AppError::Validation(format!(
                "symlink in bare repo tree: {}",
                dir.display()
            )));
        }
        if !meta.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|e| AppError::Other(e.to_string()))? {
            let entry = entry.map_err(|e| AppError::Other(e.to_string()))?;
            let p = entry.path();
            let em = fs::symlink_metadata(&p).map_err(|e| AppError::Other(e.to_string()))?;
            if em.file_type().is_symlink() {
                return Err(AppError::Validation(format!(
                    "nested symlink under bare repo: {}",
                    p.display()
                )));
            }
            if em.is_dir() {
                stack.push(p);
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn collect_bare_repos_under_root(git_root: &Path) -> Result<Vec<PathBuf>, AppError> {
    reject_symlink_path(git_root)?;
    let root_meta = fs::symlink_metadata(git_root).map_err(|e| AppError::Other(e.to_string()))?;
    if root_meta.file_type().is_symlink() {
        return Err(AppError::Validation(
            "git storage root must not be a symlink".into(),
        ));
    }
    if !root_meta.is_dir() {
        return Err(AppError::Validation(
            "git storage root must be a directory".into(),
        ));
    }
    let root_canon = fs::canonicalize(git_root).map_err(|e| AppError::Other(e.to_string()))?;
    let mut bare_paths = Vec::new();
    for entry in fs::read_dir(git_root).map_err(|e| AppError::Other(e.to_string()))? {
        let entry = entry.map_err(|e| AppError::Other(e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".git") {
            continue;
        }
        let child = entry.path();
        let meta = fs::symlink_metadata(&child).map_err(|e| AppError::Other(e.to_string()))?;
        if meta.file_type().is_symlink() {
            return Err(AppError::Validation(format!(
                "bare repo entry must not be a symlink: {name}"
            )));
        }
        if !meta.is_dir() {
            continue;
        }
        let canon = fs::canonicalize(&child).map_err(|e| AppError::Other(e.to_string()))?;
        if !canon.starts_with(&root_canon) {
            return Err(AppError::Validation(
                "bare repo path escapes git storage root".into(),
            ));
        }
        preflight_bare_repo_tree(&canon)?;
        bare_paths.push(canon);
    }
    Ok(bare_paths)
}

/// Idempotent shared-git volume layout for existing named volumes (fail-closed).
#[cfg(unix)]
pub fn ensure_shared_git_storage_root(git_root: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::MetadataExt;
    if !git_root.exists() {
        return Ok(());
    }
    let group = git_storage_group();
    let expected_gid = resolve_group_gid(&group)?;
    let bare_repos = collect_bare_repos_under_root(git_root)?;
    let path = git_root
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 path".into()))?;
    let root_chgrp = Command::new("chgrp")
        .args([&group, path])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !root_chgrp {
        let sudo_ok = if storage_sudo_fallback_enabled() {
            run_privileged("chgrp", &[&group, path]).is_ok()
        } else {
            false
        };
        if !sudo_ok {
            return Err(AppError::Other(format!(
                "chgrp {group} on git storage root failed — join {group} or set MUSEHUB_GIT_STORAGE_SUDO_FALLBACK=1 for host CI only"
            )));
        }
    }
    run_storage_chmod("2775", path)?;
    let meta = fs::metadata(git_root).map_err(|e| AppError::Other(e.to_string()))?;
    if meta.gid() != expected_gid {
        return Err(AppError::Validation(format!(
            "git storage root gid {0} != expected {expected_gid} ({group})",
            meta.gid()
        )));
    }
    let mut errors = Vec::new();
    for bare in bare_repos {
        if let Err(e) = crate::git_http::apply_bare_repo_group_acl(&bare) {
            errors.push(format!("{}: {e}", bare.display()));
        }
    }
    if !errors.is_empty() {
        return Err(AppError::Other(format!(
            "bare repo migration failed for {} repo(s): {}",
            errors.len(),
            errors.join("; ")
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn ensure_shared_git_storage_root(_git_root: &Path) -> Result<(), AppError> {
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
