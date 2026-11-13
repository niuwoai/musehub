//! Git login home must not allow ~/.ssh/environment overrides.

mod common;

use musehub_app::git_ssh_runtime::{verify_git_ssh_login_home, verify_git_ssh_passwd_home};

#[test]
fn verify_rejects_sshd_environment_file() {
    common::git_ssh_sshd::ensure_git_os_user();
    let home = tempfile::tempdir().expect("home");
    std::env::set_var(
        "MUSEHUB_GIT_SSH_LOGIN_HOME",
        home.path().to_string_lossy().as_ref(),
    );
    let home_s = home.path().to_str().unwrap();
    let script = format!(
        "install -d -m 0555 -o root -g root {home_s} && \
         install -d -m 0555 -o root -g root {home_s}/.ssh && \
         echo 'MUSEHUB_SSH_KEY_ID=bad' | tee {home_s}/.ssh/environment >/dev/null"
    );
    let status = std::process::Command::new("sudo")
        .args(["bash", "-c", &script])
        .status()
        .expect("sudo");
    assert!(status.success());
    let env_file = home.path().join(".ssh/environment");
    assert!(
        env_file.is_file(),
        "test setup: missing {}",
        env_file.display()
    );
    let err = verify_git_ssh_login_home().unwrap_err();
    assert!(err.to_string().contains("environment"));
}

#[test]
fn locked_root_owned_home_passes_verify() {
    common::git_ssh_sshd::ensure_git_os_user();
    let home = tempfile::tempdir().expect("home");
    let home_s = home.path().to_str().unwrap();
    let status = std::process::Command::new("sudo")
        .args([
            "install", "-d", "-m", "0555", "-o", "root", "-g", "root", home_s,
        ])
        .status()
        .expect("sudo install");
    assert!(status.success());
    let prev = std::env::var("MUSEHUB_GIT_SSH_LOGIN_HOME").ok();
    std::env::set_var("MUSEHUB_GIT_SSH_LOGIN_HOME", home_s);
    let r = verify_git_ssh_login_home();
    if let Some(p) = prev {
        std::env::set_var("MUSEHUB_GIT_SSH_LOGIN_HOME", p);
    } else {
        std::env::remove_var("MUSEHUB_GIT_SSH_LOGIN_HOME");
    }
    r.expect("locked home");
}

#[test]
fn verify_passwd_home_rejects_when_getent_does_not_match_trusted_path() {
    common::git_ssh_sshd::ensure_git_os_user();
    let trusted = tempfile::tempdir().expect("trusted");
    std::env::set_var(
        "MUSEHUB_GIT_SSH_LOGIN_HOME",
        trusted.path().to_string_lossy().as_ref(),
    );
    let trusted_s = trusted.path().to_str().unwrap();
    let st = std::process::Command::new("sudo")
        .args([
            "install", "-d", "-m", "0555", "-o", "root", "-g", "root", trusted_s,
        ])
        .status()
        .expect("install");
    assert!(st.success());
    let passwd = musehub_app::git_ssh_runtime::passwd_home_for("git").unwrap_or_default();
    if passwd == trusted_s {
        return;
    }
    let err = verify_git_ssh_passwd_home("git").unwrap_err();
    assert!(err.to_string().contains("passwd home"), "unexpected: {err}");
}
