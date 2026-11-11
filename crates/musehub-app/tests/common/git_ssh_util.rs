use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn git_ssh_binary() -> PathBuf {
    if let Ok(p) = std::env::var("MUSEHUB_GIT_SSH_BIN") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/musehub-git-ssh")
}

pub fn locate_sshd() -> Option<PathBuf> {
    for p in ["/usr/sbin/sshd", "/usr/local/sbin/sshd"] {
        let path = Path::new(p);
        if path.exists() {
            return Some(path.to_path_buf());
        }
    }
    Command::new("which")
        .arg("sshd")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
}

/// When `MUSEHUB_REQUIRE_GIT_SSH=1` (CI), missing sshd or binary is a hard failure.
pub fn require_git_ssh_transport_prereqs() {
    let require = std::env::var("MUSEHUB_REQUIRE_GIT_SSH").ok().as_deref() == Some("1");
    let bin = git_ssh_binary();
    let sshd = locate_sshd();
    if require {
        assert!(
            sshd.is_some(),
            "MUSEHUB_REQUIRE_GIT_SSH=1 but openssh-server (sshd) is not installed"
        );
        assert!(
            bin.exists(),
            "MUSEHUB_REQUIRE_GIT_SSH=1 but musehub-git-ssh binary missing at {}",
            bin.display()
        );
    } else if sshd.is_none() || !bin.exists() {
        eprintln!(
            "git_ssh_transport=not_run reason=missing_prereq sshd={} bin_exists={}",
            sshd.is_some(),
            bin.exists()
        );
        std::process::exit(0);
    }
}

pub fn ssh_base_args(priv_key: &Path, port: u16) -> Vec<String> {
    vec![
        "-o".into(),
        "StrictHostKeyChecking=no".into(),
        "-o".into(),
        "UserKnownHostsFile=/dev/null".into(),
        "-i".into(),
        priv_key.display().to_string(),
        "-p".into(),
        port.to_string(),
    ]
}

pub fn git_ssh_cmd(priv_key: &Path, port: u16, git_args: &[&str]) -> Output {
    git_ssh_cmd_in(priv_key, port, None, git_args)
}

pub fn git_ssh_cmd_in(
    priv_key: &Path,
    port: u16,
    workdir: Option<&Path>,
    git_args: &[&str],
) -> Output {
    let ssh_cmd = format!(
        "ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {} -p {}",
        priv_key.display(),
        port
    );
    let mut cmd = Command::new("git");
    if let Some(d) = workdir {
        cmd.current_dir(d);
    }
    cmd.arg("-c").arg(format!("core.sshCommand={ssh_cmd}"));
    cmd.args(git_args);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.output().expect("git")
}

pub fn raw_ssh(priv_key: &Path, port: u16, remote_cmd: &str, allocate_tty: bool) -> Output {
    raw_ssh_as("git", priv_key, port, remote_cmd, allocate_tty)
}

pub fn raw_ssh_with_sendenv(
    priv_key: &Path,
    port: u16,
    var: &str,
    value: &str,
    remote_cmd: &str,
) -> Output {
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_base_args(priv_key, port));
    cmd.arg("-o").arg(format!("SendEnv={var}"));
    cmd.env(var, value);
    cmd.arg("git@127.0.0.1");
    if !remote_cmd.is_empty() {
        cmd.arg(remote_cmd);
    }
    cmd.output().expect("ssh sendenv")
}

pub fn raw_ssh_as(
    login: &str,
    priv_key: &Path,
    port: u16,
    remote_cmd: &str,
    allocate_tty: bool,
) -> Output {
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_base_args(priv_key, port));
    if allocate_tty {
        cmd.arg("-tt");
    }
    cmd.arg(format!("{login}@127.0.0.1"));
    if !remote_cmd.is_empty() {
        cmd.arg(remote_cmd);
    }
    cmd.output().expect("ssh")
}

pub fn ed25519_keypair(prefix: &str) -> (PathBuf, String) {
    let dir = tempfile::tempdir().expect("keydir");
    let priv_path = dir.path().join(format!("{prefix}_ed25519"));
    let status = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            priv_path.to_str().expect("path"),
            "-N",
            "",
            "-q",
        ])
        .status()
        .expect("ssh-keygen");
    assert!(status.success());
    let pub_body = std::fs::read_to_string(priv_path.with_extension("pub")).expect("pub");
    let _keep_dir = dir.keep();
    (priv_path, pub_body.trim().to_string())
}
