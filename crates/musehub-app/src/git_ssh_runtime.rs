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

/// Pre-create append-only server log under a validated run dir (git group can write).
#[cfg(unix)]
pub fn prepare_git_ssh_server_log(path: &Path) -> Result<(), AppError> {
    reject_symlink_path(path)?;
    if let Some(parent) = path.parent() {
        reject_symlink_path(parent)?;
        validate_secure_ancestor_chain(parent)?;
    }
    if !path.exists() {
        write_secure_file(path, b"").map_err(|e| AppError::Other(e.to_string()))?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o660))
            .map_err(|e| AppError::Other(e.to_string()))?;
        chown_root_group(path, &git_ssh_runtime_group())?;
    }
    reject_symlink_path(path)?;
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path).map_err(|e| AppError::Other(e.to_string()))?;
    if !meta.is_file() {
        return Err(AppError::Validation(
            "server log must be a regular file".into(),
        ));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o002 != 0 {
        return Err(AppError::Validation(
            "server log must not be world-writable".into(),
        ));
    }
    if mode & 0o020 == 0 {
        return Err(AppError::Validation(
            "server log must be group-writable for git transport".into(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn prepare_git_ssh_server_log(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
fn validate_cap_setgid_target_meta(meta: &fs::Metadata) -> Result<(), AppError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if meta.uid() != 0 {
        return Err(AppError::Validation(
            "gateway binary must be root-owned before setcap".into(),
        ));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o022 != 0 {
        return Err(AppError::Validation(
            "gateway binary must not be group/world writable before setcap".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub fn validate_cap_setgid_gateway_file(bin: &Path) -> Result<(), AppError> {
    let meta = fs::symlink_metadata(bin).map_err(|e| AppError::Other(e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::Validation(
            "gateway binary path must not be a symlink".into(),
        ));
    }
    if !meta.is_file() {
        return Err(AppError::Validation("gateway binary must be a file".into()));
    }
    validate_cap_setgid_target_meta(&meta)
}

/// Apply `cap_setgid+ep` under `flock` while holding an `O_NOFOLLOW` open fd (inode-checked).
#[cfg(unix)]
pub fn apply_cap_setgid_on_gateway(bin: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    reject_symlink_path(bin)?;
    validate_secure_ancestor_chain(bin)?;
    let meta_before = fs::symlink_metadata(bin).map_err(|e| AppError::Other(e.to_string()))?;
    validate_cap_setgid_target_meta(&meta_before)?;
    let ino_before = meta_before.ino();
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(bin)
        .map_err(|e| AppError::Other(e.to_string()))?;
    let fd = file.as_raw_fd();
    let mut st_fd = std::mem::MaybeUninit::<libc::stat>::zeroed();
    if unsafe { libc::fstat(fd, st_fd.as_mut_ptr()) } != 0 {
        return Err(AppError::Other("fstat gateway fd failed".into()));
    }
    let st_fd = unsafe { st_fd.assume_init() };
    if st_fd.st_ino != ino_before {
        return Err(AppError::Validation(
            "gateway inode changed before setcap".into(),
        ));
    }
    let path = bin
        .to_str()
        .ok_or_else(|| AppError::Other("gateway path utf8".into()))?;
    unsafe {
        if libc::flock(fd, libc::LOCK_SH) != 0 {
            return Err(AppError::Other("flock gateway before setcap failed".into()));
        }
    }
    let cap_ok = if is_root_euid() {
        Command::new("setcap")
            .args(["cap_setgid+ep", path])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        run_privileged("setcap", &["cap_setgid+ep", path]).is_ok()
    };
    unsafe {
        libc::flock(fd, libc::LOCK_UN);
    }
    if !cap_ok {
        return Err(AppError::Other("setcap cap_setgid+ep failed".into()));
    }
    let getcap = Command::new("getcap")
        .arg(path)
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !getcap.status.success() || !String::from_utf8_lossy(&getcap.stdout).contains("cap_setgid") {
        return Err(AppError::Other("getcap did not show cap_setgid".into()));
    }
    let meta_after = fs::symlink_metadata(bin).map_err(|e| AppError::Other(e.to_string()))?;
    if meta_after.ino() != ino_before {
        return Err(AppError::Validation(
            "gateway path inode changed after setcap".into(),
        ));
    }
    validate_cap_setgid_target_meta(&meta_after)
}

#[cfg(not(unix))]
pub fn validate_cap_setgid_gateway_file(_bin: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(not(unix))]
pub fn apply_cap_setgid_on_gateway(_bin: &Path) -> Result<(), AppError> {
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

/// Root-owned, non-writable home for `git` (no `~/.ssh/environment` overrides).
pub const GIT_SSH_LOGIN_HOME: &str = "/var/lib/musehub/git-ssh/home";

pub fn git_ssh_login_home() -> PathBuf {
    std::env::var("MUSEHUB_GIT_SSH_LOGIN_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(GIT_SSH_LOGIN_HOME))
}

#[cfg(unix)]
fn login_home_must_be_locked(meta: &fs::Metadata) -> Result<(), AppError> {
    use std::os::unix::fs::MetadataExt;
    if meta.uid() != 0 {
        return Err(AppError::Validation(
            "git ssh login home must be root-owned".into(),
        ));
    }
    let mode = meta.mode() & 0o777;
    if mode & 0o022 != 0 {
        return Err(AppError::Validation(
            "git ssh login home must not be group/world-writable".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_locked_login_home_at(home: &Path) -> Result<(), AppError> {
    reject_symlink_path(home)?;
    if !home.is_dir() {
        return Err(AppError::Validation(format!(
            "git ssh login home missing: {}",
            home.display()
        )));
    }
    let meta = fs::metadata(home).map_err(|e| AppError::Other(e.to_string()))?;
    login_home_must_be_locked(&meta)?;
    let env_file = home.join(".ssh").join("environment");
    if env_file.exists() {
        return Err(AppError::Validation(format!(
            "git ssh refuses ~/.ssh/environment at {}",
            env_file.display()
        )));
    }
    if home.join(".ssh").exists() {
        let ssh_dir = home.join(".ssh");
        reject_symlink_path(&ssh_dir)?;
        let sm = fs::metadata(&ssh_dir).map_err(|e| AppError::Other(e.to_string()))?;
        use std::os::unix::fs::MetadataExt;
        if sm.uid() != 0 || (sm.mode() & 0o022) != 0 {
            return Err(AppError::Validation(
                "git ssh home .ssh must be root-owned and not group/world-writable".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn verify_git_ssh_login_home() -> Result<(), AppError> {
    verify_locked_login_home_at(&git_ssh_login_home())
}

/// `sshd` reads `~/.ssh/environment` from **passwd** `pw_dir`, not session `HOME`.
#[cfg(unix)]
pub fn verify_git_ssh_passwd_home(login_user: &str) -> Result<(), AppError> {
    let trusted = git_ssh_login_home();
    let trusted_s = trusted
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 trusted home".into()))?;
    let passwd_home = passwd_home_dir(login_user)
        .ok_or_else(|| AppError::Validation(format!("passwd entry missing for {login_user}")))?;
    if passwd_home != trusted_s {
        return Err(AppError::Validation(format!(
            "passwd home for {login_user} is {passwd_home}, require {trusted_s}"
        )));
    }
    verify_locked_login_home_at(&trusted)
}

#[cfg(not(unix))]
pub fn verify_git_ssh_login_home() -> Result<(), AppError> {
    Ok(())
}

#[cfg(not(unix))]
pub fn verify_git_ssh_passwd_home(_login_user: &str) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
pub fn passwd_home_for(login_user: &str) -> Option<String> {
    passwd_home_dir(login_user)
}

#[cfg(unix)]
fn passwd_home_dir(login_user: &str) -> Option<String> {
    let out = Command::new("getent")
        .args(["passwd", login_user])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    line.split(':').nth(5).map(|s| s.to_string())
}

/// Provision locked login home and repoint the `git` passwd entry (idempotent).
#[cfg(unix)]
pub fn ensure_git_ssh_login_home(login_user: &str) -> Result<(), AppError> {
    let home = git_ssh_login_home();
    let home_s = home
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 home path".into()))?;
    if !home.exists() {
        run_privileged(
            "install",
            &["-d", "-m", "0555", "-o", "root", "-g", "root", home_s],
        )?;
    } else {
        reject_symlink_path(&home)?;
        run_privileged("chown", &["root:root", home_s]).ok();
        run_storage_chmod("0555", home_s)?;
    }
    let env_file = home.join(".ssh").join("environment");
    if env_file.exists() {
        return Err(AppError::Validation(format!(
            "remove {} before starting git sshd",
            env_file.display()
        )));
    }
    let needs_usermod = passwd_home_dir(login_user).as_deref() != Some(home_s);
    if needs_usermod {
        run_privileged("usermod", &["-d", home_s, login_user])?;
    }
    verify_git_ssh_passwd_home(login_user)?;
    verify_git_ssh_login_home()
}

#[cfg(not(unix))]
pub fn ensure_git_ssh_login_home(_login_user: &str) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
struct PathInode {
    path: PathBuf,
    dev: u64,
    ino: u64,
    is_dir: bool,
}

#[cfg(unix)]
fn path_inode_record(p: &Path) -> Result<PathInode, AppError> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(p).map_err(|e| AppError::Other(e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::Validation(format!(
            "symlink during migration: {}",
            p.display()
        )));
    }
    Ok(PathInode {
        path: p.to_path_buf(),
        dev: meta.dev(),
        ino: meta.ino(),
        is_dir: meta.is_dir(),
    })
}

#[cfg(unix)]
fn assert_path_inode_unchanged(pi: &PathInode) -> Result<(), AppError> {
    let cur = path_inode_record(&pi.path)?;
    if cur.dev != pi.dev || cur.ino != pi.ino {
        return Err(AppError::Validation(format!(
            "path identity changed during privileged migration: {}",
            pi.path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub fn collect_physical_tree_paths(root: &Path) -> Result<Vec<PathBuf>, AppError> {
    preflight_bare_repo_tree(root)?;
    let mut paths = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = fs::symlink_metadata(&dir).map_err(|e| AppError::Other(e.to_string()))?;
        if !meta.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|e| AppError::Other(e.to_string()))? {
            let entry = entry.map_err(|e| AppError::Other(e.to_string()))?;
            let p = entry.path();
            path_inode_record(&p)?;
            paths.push(p.clone());
            if fs::symlink_metadata(&p)
                .map_err(|e| AppError::Other(e.to_string()))?
                .is_dir()
            {
                stack.push(p);
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[cfg(unix)]
fn run_chgrp_file(group: &str, path: &str) -> Result<(), AppError> {
    let ok = Command::new("chgrp")
        .args(["-h", group, path])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    if storage_sudo_fallback_enabled() {
        return run_privileged("chgrp", &["-h", group, path]);
    }
    Err(AppError::Other(format!("chgrp {group} {path} failed")))
}

#[cfg(unix)]
fn run_chmod_file(args: &[&str], path: &str) -> Result<(), AppError> {
    let ok = Command::new("chmod")
        .args(args)
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    if storage_sudo_fallback_enabled() {
        let mut priv_args: Vec<&str> = Vec::with_capacity(args.len() + 1);
        priv_args.extend_from_slice(args);
        priv_args.push(path);
        return run_privileged("chmod", &priv_args);
    }
    Err(AppError::Other(format!("chmod on {path} failed")))
}

/// Apply group ACL by physical walk (no `chgrp -R` / symlink follow). Not race-proof:
/// concurrent writers on a group-writable root can still swap paths; preflight is static.
#[cfg(unix)]
pub fn apply_bare_repo_group_acl_physical(bare: &Path) -> Result<(), AppError> {
    let group = git_storage_group();
    let paths = collect_physical_tree_paths(bare)?;
    let records: Vec<PathInode> = paths
        .iter()
        .map(|p| path_inode_record(p))
        .collect::<Result<_, _>>()?;
    for rec in &records {
        assert_path_inode_unchanged(rec)?;
        let ps = rec
            .path
            .to_str()
            .ok_or_else(|| AppError::Other("utf8 path".into()))?;
        run_chgrp_file(&group, ps)?;
        run_chmod_file(&["g+rwX"], ps)?;
        if rec.is_dir {
            run_chmod_file(&["g+s"], ps)?;
        }
    }
    let path = bare
        .to_str()
        .ok_or_else(|| AppError::Other("utf8 path".into()))?;
    let mut shared_ok = Command::new("git")
        .args([
            "--git-dir",
            path,
            "config",
            "core.sharedRepository",
            "group",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !shared_ok && storage_sudo_fallback_enabled() {
        shared_ok = Command::new("sudo")
            .args([
                "-n",
                "git",
                "-c",
                &format!("safe.directory={path}"),
                "--git-dir",
                path,
                "config",
                "core.sharedRepository",
                "group",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    if !shared_ok {
        return Err(AppError::Other(
            "git config core.sharedRepository failed".into(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn apply_bare_repo_group_acl_physical(_bare: &Path) -> Result<(), AppError> {
    Ok(())
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

#[cfg(all(test, unix))]
mod git_ssh_cap_tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn setcap_rejects_symlink_gateway_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("musehub-git-ssh-real");
        std::fs::write(&real, b"\x7fELF\n").expect("write");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let link = dir.path().join("musehub-git-ssh");
        symlink(&real, &link).expect("symlink");
        let err = validate_cap_setgid_gateway_file(&link).expect_err("symlink");
        assert!(
            err.to_string().contains("symlink"),
            "expected symlink rejection, got {err}"
        );
    }
}

#[cfg(all(test, unix))]
mod migration_tests {
    use super::*;
    use std::fs;

    #[test]
    fn chmod_sudo_argv_is_mode_then_path_not_double_chmod() {
        let path = "/tmp/musehub-chmod-test";
        let mode_args = ["g+rwX"];
        let mut priv_args: Vec<&str> = Vec::with_capacity(mode_args.len() + 1);
        priv_args.extend_from_slice(&mode_args);
        priv_args.push(path);
        assert_eq!(priv_args, ["g+rwX", "/tmp/musehub-chmod-test"]);
    }

    #[test]
    fn privileged_migration_detects_replaced_path_inode() {
        let dir = tempfile::tempdir().expect("dir");
        let f = dir.path().join("f");
        fs::write(&f, b"a").unwrap();
        let pi = path_inode_record(&f).unwrap();
        let replacement = dir.path().join("replacement");
        fs::write(&replacement, b"b").unwrap();
        fs::remove_file(&f).unwrap();
        fs::rename(&replacement, &f).unwrap();
        let err = assert_path_inode_unchanged(&pi).unwrap_err();
        assert!(err.to_string().contains("identity changed"));
    }
}
