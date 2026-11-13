use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

type PipedChild = (Child, mpsc::Receiver<Vec<u8>>, mpsc::Receiver<Vec<u8>>);

#[derive(Debug, Clone)]
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

    pub fn stderr_lossy_redacted(&self) -> String {
        redact_test_output(&String::from_utf8_lossy(&self.stderr))
    }
}

fn ssh_timeout_secs() -> u64 {
    std::env::var("MUSEHUB_TEST_SSH_TIMEOUT_SEC")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30)
}

pub fn git_timeout_secs() -> u64 {
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

#[cfg(unix)]
fn configure_new_session(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            let _ = libc::setsid();
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_new_session(_cmd: &mut Command) {}

const KILL_GRACE_SECS: u64 = 2;
const PIPE_DRAIN_SECS: u64 = 8;

#[cfg(unix)]
fn signal_process_group(pid: i32, sig: i32) {
    unsafe {
        let _ = libc::kill(-pid, sig);
    }
}

#[cfg(unix)]
fn terminate_child_group(child: &mut Child) {
    let pid = child.id() as i32;
    signal_process_group(pid, libc::SIGTERM);
    let grace = Instant::now() + Duration::from_secs(KILL_GRACE_SECS);
    while Instant::now() < grace {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    signal_process_group(pid, libc::SIGKILL);
    let final_wait = Instant::now() + Duration::from_secs(KILL_GRACE_SECS);
    while Instant::now() < final_wait {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(unix))]
fn terminate_child_group(child: &mut Child) {
    let _ = child.kill();
}

fn finish_child_pipes(
    child: &mut Child,
    stdout_rx: mpsc::Receiver<Vec<u8>>,
    stderr_rx: mpsc::Receiver<Vec<u8>>,
) -> (Vec<u8>, Vec<u8>) {
    let wait_deadline = Instant::now() + Duration::from_secs(PIPE_DRAIN_SECS);
    while Instant::now() < wait_deadline {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if child.try_wait().ok().flatten().is_none() {
        terminate_child_group(child);
        let hard = Instant::now() + Duration::from_secs(KILL_GRACE_SECS + 1);
        while Instant::now() < hard {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    let stdout = stdout_rx
        .recv_timeout(Duration::from_secs(PIPE_DRAIN_SECS))
        .unwrap_or_default();
    let stderr = stderr_rx
        .recv_timeout(Duration::from_secs(PIPE_DRAIN_SECS))
        .unwrap_or_default();
    (stdout, stderr)
}

fn spawn_piped(mut cmd: Command) -> Result<PipedChild, String> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    configure_new_session(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout pipe".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr pipe".to_string())?;
    let (stx, srx) = mpsc::sync_channel::<Vec<u8>>(1);
    let (etx, erx) = mpsc::sync_channel::<Vec<u8>>(1);
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut r = stdout;
        r.read_to_end(&mut buf).ok();
        let _ = stx.send(buf);
    });
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut r = stderr;
        r.read_to_end(&mut buf).ok();
        let _ = etx.send(buf);
    });
    Ok((child, srx, erx))
}

/// Run subprocess with concurrent stdout/stderr drain (avoids pipe deadlock).
pub fn run_bounded(cmd: Command, timeout_secs: u64) -> BoundedOutput {
    let (mut child, srx, erx) = match spawn_piped(cmd) {
        Ok(v) => v,
        Err(e) => {
            return BoundedOutput {
                status: timed_out_status(),
                stdout: Vec::new(),
                stderr: format!("{e}\n").into_bytes(),
                timed_out: false,
            };
        }
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let (stdout, stderr) = finish_child_pipes(&mut child, srx, erx);
                return BoundedOutput {
                    status,
                    stdout,
                    stderr,
                    timed_out: false,
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    terminate_child_group(&mut child);
                    let (stdout, stderr) = finish_child_pipes(&mut child, srx, erx);
                    return BoundedOutput {
                        status: timed_out_status(),
                        stdout,
                        stderr,
                        timed_out: true,
                    };
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                terminate_child_group(&mut child);
                let (stdout, _stderr) = finish_child_pipes(&mut child, srx, erx);
                return BoundedOutput {
                    status: timed_out_status(),
                    stdout,
                    stderr: format!("wait failed: {e}\n").into_bytes(),
                    timed_out: false,
                };
            }
        }
    }
}

fn is_pat_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

pub fn redact_test_output(text: &str) -> String {
    let mut text = text.to_string();
    if let Ok(pat) = std::env::var("GIT_HTTP_SANDBOX_PAT") {
        if !pat.is_empty() {
            text = text.replace(&pat, "[REDACTED_PAT]");
            let basic = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("admin:{pat}"),
            );
            text = text.replace(&basic, "[REDACTED_BASIC]");
        }
    }
    while let Some(start) = text.find("Authorization: Basic ") {
        let rest = &text[start + "Authorization: Basic ".len()..];
        let len = rest
            .chars()
            .take_while(|c| *c == '=' || c.is_ascii_alphanumeric() || *c == '+' || *c == '/')
            .count();
        text.replace_range(
            start..start + "Authorization: Basic ".len() + len,
            "Authorization: Basic [REDACTED]",
        );
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(idx) = rest.find("mhp_") {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx..];
        let token_len = tail
            .chars()
            .take_while(|c| is_pat_token_char(*c))
            .count()
            .max(4);
        out.push_str("[REDACTED_PAT]");
        rest = &tail[token_len..];
    }
    out.push_str(rest);
    out
}

/// Explicit sshd/admin forward denial — not timeout, not unrelated stderr.
pub fn ssh_forward_admin_denial(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("administratively prohibited")
        || s.contains("port forwarding is disabled")
        || s.contains("forwarding disabled")
        || s.contains("open failed: administratively prohibited")
        || (s.contains("port forwarding") && s.contains("disabled"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ForwardDenialVerdict {
    Denied,
    ChannelOpened,
    Inconclusive,
    TimedOut,
}

pub fn classify_forward_denial(
    exited_before_deadline: bool,
    timed_out_kill: bool,
    forward_echo_validated: bool,
    stderr: &str,
) -> ForwardDenialVerdict {
    if forward_echo_validated {
        return ForwardDenialVerdict::ChannelOpened;
    }
    if ssh_forward_admin_denial(stderr) {
        return ForwardDenialVerdict::Denied;
    }
    if timed_out_kill || !exited_before_deadline {
        return ForwardDenialVerdict::TimedOut;
    }
    ForwardDenialVerdict::Inconclusive
}

/// Echo target for forward probe: returns bytes sent by client (one connection).
pub fn spawn_nonce_echo_target() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("target bind");
    let port = listener.local_addr().expect("target addr").port();
    let handle = thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let deadline = Instant::now() + Duration::from_secs(25);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut sock, _)) => {
                    let mut buf = [0u8; 64];
                    sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
                    sock.set_write_timeout(Some(Duration::from_secs(2))).ok();
                    match sock.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => {
                            let _ = sock.write_all(&buf[..n]);
                            let _ = sock.flush();
                        }
                        Err(_) => return,
                    }
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return,
            }
        }
    });
    (port, handle)
}

/// True only when nonce bytes echo back through `local_port` (proves direct-tcpip tunnel).
pub fn forward_channel_echo_proven(local_port: u16, nonce: &[u8]) -> bool {
    if nonce.is_empty() || nonce.len() > 48 {
        return false;
    }
    let addr: SocketAddr = format!("127.0.0.1:{local_port}")
        .parse()
        .unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap());
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(800)) else {
        return false;
    };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    if stream.write_all(nonce).is_err() {
        return false;
    }
    let mut got = vec![0u8; nonce.len()];
    if stream.read_exact(&mut got).is_err() {
        return false;
    }
    got == nonce
}

/// Product negative: forward channel must be administratively denied with TCP probe to local listener.
pub fn assert_ssh_port_forward_denied(priv_key: &Path, port: u16) {
    let (target_port, target_handle) = spawn_nonce_echo_target();
    let local_listener = TcpListener::bind("127.0.0.1:0").expect("local bind");
    let local_port = local_listener.local_addr().expect("local").port();
    drop(local_listener);
    let nonce = format!("musehub-fwd-{}", std::process::id());
    let nonce_bytes = nonce.as_bytes();

    let mut cmd = Command::new("ssh");
    cmd.args(ssh_base_args(priv_key, port));
    cmd.arg("-o").arg("ExitOnForwardFailure=yes");
    cmd.args([
        "-N",
        "-L",
        &format!("127.0.0.1:{local_port}:127.0.0.1:{target_port}"),
        "git@127.0.0.1",
    ]);

    let (mut child, srx, erx) = spawn_piped(cmd).expect("spawn forward ssh");
    let probe_deadline = Instant::now() + Duration::from_secs(12);
    let mut forward_echo_validated = false;
    let mut exited = false;
    while Instant::now() < probe_deadline {
        if let Ok(Some(_)) = child.try_wait() {
            exited = true;
            break;
        }
        if forward_channel_echo_proven(local_port, nonce_bytes) {
            forward_echo_validated = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let timed_out_kill = !exited;
    terminate_child_group(&mut child);
    let (stdout, stderr) = finish_child_pipes(&mut child, srx, erx);
    let _ = stdout;
    let _ = target_handle.join();
    let stderr_s = String::from_utf8_lossy(&stderr).into_owned();

    let verdict =
        classify_forward_denial(exited, timed_out_kill, forward_echo_validated, &stderr_s);
    assert_eq!(
        verdict,
        ForwardDenialVerdict::Denied,
        "forward negative: exited={exited} echo_ok={forward_echo_validated} stderr={}",
        redact_test_output(&stderr_s)
    );
    let addr: SocketAddr = format!("127.0.0.1:{local_port}").parse().expect("addr");
    assert!(
        TcpListener::bind(addr).is_ok(),
        "forward local port {local_port} must remain free"
    );
}

pub fn raw_ssh_bounded(
    login: &str,
    priv_key: &Path,
    port: u16,
    remote_cmd: &str,
    allocate_tty: bool,
) -> BoundedOutput {
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_base_args(priv_key, port));
    if allocate_tty {
        cmd.arg("-tt");
    }
    cmd.arg(format!("{login}@127.0.0.1"));
    if !remote_cmd.is_empty() {
        cmd.arg(remote_cmd);
    }
    let cap = if allocate_tty && remote_cmd.is_empty() {
        12
    } else {
        ssh_timeout_secs()
    };
    run_bounded(cmd, cap)
}

/// Negative ssh must fail with real denial before the bounded cap — never exit 124 / timeout.
pub fn raw_ssh_expect_denied(
    priv_key: &Path,
    port: u16,
    remote_cmd: &str,
    allocate_tty: bool,
) -> Output {
    let login = "git";
    let b = raw_ssh_bounded(login, priv_key, port, remote_cmd, allocate_tty);
    assert!(
        !b.timed_out,
        "ssh {:?} must be denied before timeout (tty={allocate_tty}): {}",
        remote_cmd,
        b.stderr_lossy_redacted()
    );
    assert!(
        !b.status.success(),
        "ssh {:?} must not succeed: {}",
        remote_cmd,
        b.stderr_lossy_redacted()
    );
    if allocate_tty && remote_cmd.is_empty() {
        assert!(
            !b.stderr.is_empty() || b.status.code() == Some(255),
            "interactive shell refusal must be visible in exit status/stderr"
        );
    }
    b.into_output()
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
    git_ssh_cmd_in_bounded(priv_key, port, workdir, git_args).into_output()
}

pub fn git_ssh_cmd_in_bounded(
    priv_key: &Path,
    port: u16,
    workdir: Option<&Path>,
    git_args: &[&str],
) -> BoundedOutput {
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
    run_bounded(cmd, git_timeout_secs())
}

pub fn format_bounded_git_transport_failure(label: &str, b: &BoundedOutput) -> String {
    let stdout = String::from_utf8_lossy(&b.stdout);
    let stderr = redact_test_output(&String::from_utf8_lossy(&b.stderr));
    let code = b.status.code();
    if b.timed_out {
        format!(
            "{label}: timed out after {}s waiting for remote (receive-pack may have hung — check gateway stderr); exit={code:?}; stdout={stdout}; stderr={stderr}",
            git_timeout_secs(),
        )
    } else {
        format!("{label}: exit={code:?}; stdout={stdout}; stderr={stderr}")
    }
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
    run_bounded(cmd, ssh_timeout_secs()).into_output()
}

pub fn raw_ssh_as_expect_denied(
    login: &str,
    priv_key: &Path,
    port: u16,
    remote_cmd: &str,
    allocate_tty: bool,
) -> Output {
    let b = raw_ssh_bounded(login, priv_key, port, remote_cmd, allocate_tty);
    assert!(
        !b.timed_out,
        "ssh as {login} {:?} must be denied before timeout: {}",
        remote_cmd,
        b.stderr_lossy_redacted()
    );
    assert!(!b.status.success());
    b.into_output()
}

pub fn raw_ssh_as(
    login: &str,
    priv_key: &Path,
    port: u16,
    remote_cmd: &str,
    allocate_tty: bool,
) -> Output {
    raw_ssh_bounded(login, priv_key, port, remote_cmd, allocate_tty).into_output()
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
