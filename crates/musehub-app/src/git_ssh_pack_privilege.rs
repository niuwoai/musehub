//! Linux privilege boundary for Git-over-SSH receive-pack (storage GID + capability drop).

use std::io;
use std::process::Command;

use crate::error::AppError;
use crate::git_ssh_runtime::git_storage_group;

const FORBIDDEN_STORAGE_GROUP_NAMES: &[&str] = &[
    "root", "wheel", "sudo", "adm", "docker", "shadow", "disk", "staff", "tty", "kmem", "tape",
    "daemon", "bin", "sys", "games", "man", "lp", "mail", "news", "uucp", "proxy", "dialout",
    "fax", "voice", "cdrom", "floppy", "audio", "dip", "plugdev", "netdev", "lxd",
];

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
    clear_linux_capabilities()
        .map_err(|e| AppError::Other(format!("drop gateway capabilities: {e}")))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn drop_gateway_inherited_capabilities() -> Result<(), AppError> {
    Ok(())
}

/// Runs in the receive-pack child immediately before `exec git` (no caps, fixed GIDs).
#[cfg(unix)]
pub fn receive_pack_child_privilege_setup(gid: libc::gid_t) -> io::Result<()> {
    unsafe {
        if libc::setresgid(gid, gid, gid) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::setresuid(libc::getuid(), libc::getuid(), libc::getuid()) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        );
        clear_linux_capabilities()?;
    }
    write_privilege_probe_file_if_configured()?;
    Ok(())
}

#[cfg(unix)]
fn write_privilege_probe_file_if_configured() -> io::Result<()> {
    let Ok(path) = std::env::var("MUSEHUB_GIT_SSH_PRIVILEGE_PROBE_FILE") else {
        return Ok(());
    };
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    let mut rgid = 0;
    let mut egid = 0;
    let mut sgid = 0;
    unsafe {
        if libc::getresgid(&mut rgid, &mut egid, &mut sgid) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let cap_eff = status
        .lines()
        .find(|l| l.starts_with("CapEff:"))
        .unwrap_or("CapEff:\t?")
        .to_string();
    let body = format!("rgid={rgid} egid={egid} sgid={sgid}\n{cap_eff}\n");
    std::fs::write(path, body)
}

#[cfg(not(unix))]
pub fn receive_pack_child_privilege_setup(_gid: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
fn clear_linux_capabilities() -> io::Result<()> {
    capset_empty()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn clear_linux_capabilities() -> io::Result<()> {
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
fn capset_empty() -> io::Result<()> {
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
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
pub fn effective_capability_mask_is_empty() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("CapEff:"))
                .map(|l| l.split_whitespace().nth(1).unwrap_or("1").to_string())
        })
        .map(|v| v.trim() == "0000000000000000")
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn effective_capability_mask_is_empty() -> bool {
    true
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
    fn receive_pack_child_pins_gid_and_drops_cap_eff() {
        let gid = match resolve_trusted_storage_group_gid() {
            Ok(gid) => gid,
            Err(_) => {
                eprintln!("skip: storage group/git membership not configured on host");
                return;
            }
        };
        unsafe {
            let pid = libc::fork();
            assert!(pid >= 0, "fork");
            if pid == 0 {
                if receive_pack_child_privilege_setup(gid).is_err() {
                    libc::_exit(77);
                }
                if !effective_capability_mask_is_empty() {
                    libc::_exit(78);
                }
                let mut rgid = 0;
                let mut egid = 0;
                let mut sgid = 0;
                if libc::getresgid(&mut rgid, &mut egid, &mut sgid) != 0
                    || (rgid, egid, sgid) != (gid, gid, gid)
                {
                    libc::_exit(79);
                }
                libc::_exit(0);
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
            assert!(libc::WIFEXITED(status));
            match libc::WEXITSTATUS(status) {
                77 => {
                    eprintln!("skip: setresgid requires cap_setgid file capability on gateway");
                }
                78 => panic!("CapEff must be zero before exec git"),
                79 => panic!("rgid/egid/sgid must match storage group"),
                0 => {}
                other => panic!("unexpected child exit {other}"),
            }
        }
    }
}
