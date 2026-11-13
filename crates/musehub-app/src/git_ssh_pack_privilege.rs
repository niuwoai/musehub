//! Linux privilege boundary for Git-over-SSH receive-pack (storage GID + capability drop).

use std::process::Command;

use crate::error::AppError;
use crate::git_ssh_runtime::git_storage_group;

const FORBIDDEN_STORAGE_GROUP_NAMES: &[&str] = &[
    "root", "wheel", "sudo", "adm", "docker", "shadow", "disk", "staff", "tty", "kmem", "tape",
    "daemon", "bin", "sys", "games", "man", "lp", "mail", "news", "uucp", "proxy", "dialout",
    "fax", "voice", "cdrom", "floppy", "audio", "dip", "plugdev", "netdev", "lxd",
];

/// Precomputed IDs for the receive-pack child `pre_exec` hook (async-signal-safe only).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ReceivePackChildSetup {
    pub storage_gid: libc::gid_t,
    pub git_uid: libc::uid_t,
}

/// Resolves the configured storage group GID after fail-closed policy checks.
#[cfg(unix)]
pub fn resolve_trusted_storage_group_gid() -> Result<libc::gid_t, AppError> {
    let group = git_storage_group();
    validate_storage_group_name(&group)?;
    let gid = storage_group_gid_from_getent(&group)?;
    if gid == 0 {
        return Err(AppError::Other(
            "refusing storage group gid 0 (privileged)".into(),
        ));
    }
    let privileged = std::fs::read_to_string("/etc/group").ok();
    if let Some(privileged) = privileged {
        if let Some(root_line) = privileged.lines().find(|l| l.starts_with("root:")) {
            if let Some(root_gid) = root_line
                .split(':')
                .nth(2)
                .and_then(|s| s.trim().parse::<u32>().ok())
            {
                if gid == root_gid {
                    return Err(AppError::Other(
                        "refusing storage group gid matching root".into(),
                    ));
                }
            }
        }
    }
    ensure_git_login_user_in_group(&group)?;
    Ok(gid)
}

#[cfg(not(unix))]
pub fn resolve_trusted_storage_group_gid() -> Result<u32, AppError> {
    Err(AppError::Other(
        "storage group receive-pack requires unix".into(),
    ))
}

#[cfg(unix)]
pub fn resolve_git_transport_passwd_uid() -> Result<libc::uid_t, AppError> {
    let user = crate::git_ssh::GIT_SSH_LOGIN_USER;
    let out = Command::new("getent")
        .args(["passwd", user])
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !out.status.success() {
        return Err(AppError::Other(format!("passwd entry for {user} missing")));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let uid = line
        .split(':')
        .nth(2)
        .and_then(|s| s.trim().parse::<libc::uid_t>().ok())
        .ok_or_else(|| AppError::Other(format!("invalid passwd uid for {user}")))?;
    if uid == 0 {
        return Err(AppError::Other(format!("refusing uid 0 for {user}")));
    }
    Ok(uid)
}

#[cfg(unix)]
pub fn prepare_receive_pack_child_setup() -> Result<ReceivePackChildSetup, AppError> {
    let storage_gid = resolve_trusted_storage_group_gid()?;
    let git_uid = resolve_git_transport_passwd_uid()?;
    let ruid = unsafe { libc::getuid() };
    if ruid != git_uid {
        return Err(AppError::Other(format!(
            "git transport uid mismatch: session ruid={ruid} passwd uid={git_uid}"
        )));
    }
    Ok(ReceivePackChildSetup {
        storage_gid,
        git_uid,
    })
}

#[cfg(unix)]
pub fn validate_storage_group_name(group: &str) -> Result<(), AppError> {
    let trimmed = group.trim();
    if trimmed.is_empty() {
        return Err(AppError::Other("storage group name required".into()));
    }
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::Other(
            "refusing numeric-only storage group name".into(),
        ));
    }
    if FORBIDDEN_STORAGE_GROUP_NAMES
        .iter()
        .any(|deny| deny.eq_ignore_ascii_case(trimmed))
    {
        return Err(AppError::Other(format!(
            "refusing privileged storage group name {trimmed}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn storage_group_gid_from_getent(group: &str) -> Result<libc::gid_t, AppError> {
    let out = Command::new("getent")
        .args(["group", group])
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !out.status.success() {
        return Err(AppError::Other(format!("storage group {group} missing")));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let name = line.split(':').next().unwrap_or_default();
    if name != group {
        return Err(AppError::Other(format!(
            "getent group {group} name mismatch"
        )));
    }
    let gid = line
        .split(':')
        .nth(2)
        .and_then(|s| s.trim().parse::<libc::gid_t>().ok())
        .ok_or_else(|| AppError::Other(format!("getent group {group} invalid gid")))?;
    Ok(gid)
}

#[cfg(unix)]
fn ensure_git_login_user_in_group(group: &str) -> Result<(), AppError> {
    let out = Command::new("getent")
        .args(["group", group])
        .output()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !out.status.success() {
        return Err(AppError::Other(format!("storage group {group} missing")));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let members = line.split(':').nth(3).unwrap_or_default();
    let git_user = crate::git_ssh::GIT_SSH_LOGIN_USER;
    let ok = members.split(',').any(|m| m.trim() == git_user);
    if !ok {
        return Err(AppError::Other(format!(
            "user {git_user} must be a member of storage group {group}"
        )));
    }
    Ok(())
}

/// Drop file capabilities inherited from a `cap_setgid` binary for non-pack entrypoints.
#[cfg(unix)]
pub fn drop_gateway_inherited_capabilities() -> Result<(), AppError> {
    clear_linux_capabilities().map_err(|errno| {
        AppError::Other(format!("drop gateway capabilities failed errno={errno}"))
    })?;
    Ok(())
}

#[cfg(not(unix))]
pub fn drop_gateway_inherited_capabilities() -> Result<(), AppError> {
    Ok(())
}

/// Runs in the receive-pack child immediately before `exec git` (async-signal-safe).
///
/// # Safety
///
/// Must only be called from a `pre_exec` hook in the child process between `fork` and `exec`.
/// Caller must have precomputed `setup` in the parent; this function performs no allocation and
/// uses only async-signal-safe syscalls.
#[cfg(unix)]
pub unsafe fn receive_pack_child_privilege_setup_raw(setup: ReceivePackChildSetup) {
    let gid = setup.storage_gid;
    let uid = setup.git_uid;
    let gid_ptr = &gid as *const libc::gid_t;
    if libc::setgroups(1, gid_ptr) != 0 {
        libc::_exit(91);
    }
    if libc::setresgid(gid, gid, gid) != 0 {
        libc::_exit(92);
    }
    if libc::setresuid(uid, uid, uid) != 0 {
        libc::_exit(93);
    }
    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
        libc::_exit(94);
    }
    if libc::prctl(
        libc::PR_CAP_AMBIENT,
        libc::PR_CAP_AMBIENT_CLEAR_ALL,
        0,
        0,
        0,
    ) != 0
    {
        libc::_exit(95);
    }
    if capset_empty_raw() != 0 {
        libc::_exit(96);
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn clear_linux_capabilities() -> Result<(), i32> {
    match capset_empty_raw() {
        0 => Ok(()),
        e => Err(e),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn clear_linux_capabilities() -> Result<(), i32> {
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
fn capset_empty_raw() -> i32 {
    #[repr(C)]
    struct CapUserHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct CapUserData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    const _LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
    const CAP_USER_DATA_U32S: usize = 2;
    let mut hdr = CapUserHeader {
        version: _LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let mut data = [CapUserData {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; CAP_USER_DATA_U32S];
    let rc = unsafe { libc::syscall(libc::SYS_capset, &mut hdr, data.as_mut_ptr()) };
    if rc != 0 {
        return unsafe { *libc::__errno_location() };
    }
    0
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn rejects_numeric_only_group_name() {
        assert!(validate_storage_group_name("12345").is_err());
    }

    #[test]
    fn rejects_forbidden_group_names() {
        for name in ["root", "docker", "shadow", "sudo"] {
            assert!(
                validate_storage_group_name(name).is_err(),
                "expected deny for {name}"
            );
        }
    }

    #[test]
    #[ignore = "requires cap_setgid file capability; see git_ssh_receive_pack_privilege_hook_e2e"]
    fn receive_pack_child_pins_gid_and_drops_cap_eff() {
        let setup = prepare_receive_pack_child_setup().expect("setup");
        unsafe {
            let pid = libc::fork();
            assert!(pid >= 0, "fork");
            if pid == 0 {
                receive_pack_child_privilege_setup_raw(setup);
                libc::_exit(0);
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
            assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        }
    }
}
