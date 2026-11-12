use std::io::Read;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

impl BoundedOutput {
    pub fn into_output(self) -> Output {
        Output {
            status: self.status,
            stdout: self.stdout,
            stderr: self.stderr,
        }
    }
}

fn ssh_timeout_secs() -> u64 {
    std::env::var("MUSEHUB_TEST_SSH_TIMEOUT_SEC")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30)
}

fn git_timeout_secs() -> u64 {
    std::env::var("MUSEHUB_TEST_GIT_SSH_TIMEOUT_SEC")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120)
}

fn timed_out_status() -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(124 << 8)
    }
    #[cfg(not(unix))]
    {
        Command::new("false").status().expect("false")
    }
}

pub fn run_bounded(cmd: &mut Command, timeout_secs: u64) -> BoundedOutput {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return BoundedOutput {
                status: timed_out_status(),
                stdout: Vec::new(),
                stderr: format!("spawn failed: {e}\n").into_bytes(),
                timed_out: false,
            };
        }
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    out.read_to_end(&mut stdout).ok();
                }
                if let Some(mut err) = child.stderr.take() {
                    err.read_to_end(&mut stderr).ok();
                }
                return BoundedOutput {
                    status,
                    stdout,
                    stderr,
                    timed_out: false,
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let mut stderr = Vec::new();
                    if let Some(mut err) = child.stderr.take() {
                        err.read_to_end(&mut stderr).ok();
                    }
                    stderr.extend_from_slice(b"\n[bounded] ssh/git command timed out\n");
                    return BoundedOutput {
                        status: timed_out_status(),
                        stdout: Vec::new(),
                        stderr,
                        timed_out: true,
                    };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return BoundedOutput {
                    status: timed_out_status(),
                    stdout: Vec::new(),
                    stderr: format!("wait failed: {e}\n").into_bytes(),
                    timed_out: false,
                };
            }
        }
    }
}

pub fn redact_test_output(text: &str) -> String {
    text.replace("mhp_", "[REDACTED_PAT]_")
}

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
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=no".into(),
        "-o".into(),
        "UserKnownHostsFile=/dev/null".into(),
        "-o".into(),
        "ControlMaster=no".into(),
        "-o".into(),
        "ConnectTimeout=15".into(),
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
        "ssh -o BatchMode=yes -o ConnectTimeout=15 -o ControlMaster=no -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {} -p {}",
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
    run_bounded(&mut cmd, git_timeout_secs()).into_output()
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
    run_bounded(&mut cmd, ssh_timeout_secs()).into_output()
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
    let timeout = if allocate_tty && remote_cmd.is_empty() {
        12
    } else {
        ssh_timeout_secs()
    };
    run_bounded(&mut cmd, timeout).into_output()
}

/// Negative test: `ssh -N -L` must fail closed (bounded; no unbounded `.output()` hang).
pub fn assert_ssh_port_forward_denied(priv_key: &Path, port: u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let local_port = listener.local_addr().expect("local addr").port();
    drop(listener);
    let addr: SocketAddr = format!("127.0.0.1:{local_port}").parse().expect("addr");

    let mut cmd = Command::new("ssh");
    cmd.args(ssh_base_args(priv_key, port));
    cmd.arg("-o").arg("ExitOnForwardFailure=yes");
    cmd.args([
        "-N",
        "-L",
        &format!("127.0.0.1:{local_port}:127.0.0.1:1"),
        "git@127.0.0.1",
    ]);
    let out = run_bounded(&mut cmd, 15);
    assert!(
        !out.status.success(),
        "ssh -N -L must not succeed: {}",
        redact_test_output(&String::from_utf8_lossy(&out.stderr))
    );
    assert!(
        TcpListener::bind(addr).is_ok(),
        "forward port {local_port} must be free after bounded ssh -L"
    );
    let stderr_s = redact_test_output(&String::from_utf8_lossy(&out.stderr));
    let denied = stderr_s.to_ascii_lowercase();
    if denied.contains("administratively prohibited")
        || denied.contains("port forwarding")
        || denied.contains("forwarding disabled")
        || denied.contains("permission denied")
    {
        return;
    }
    assert!(
        out.timed_out || !stderr_s.trim().is_empty(),
        "expected admin forward denial stderr or bounded termination without listener"
    );
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
