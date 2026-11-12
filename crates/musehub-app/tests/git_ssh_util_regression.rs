//! Regression tests for bounded ssh/git test helpers.

mod common;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::{
    assert_ssh_port_forward_denied, classify_forward_denial, forward_channel_echo_proven,
    redact_test_output, run_bounded, spawn_nonce_echo_target, ForwardDenialVerdict,
};
use std::io::Write;
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn run_bounded_times_out_sleep() {
    let mut cmd = Command::new("sleep");
    cmd.arg("30");
    let out = run_bounded(cmd, 1);
    assert!(out.timed_out, "sleep 30 must hit 1s cap");
    assert!(!out.status.success());
}

#[test]
fn run_bounded_drains_large_stderr_without_deadlock() {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("yes x | head -c 262144 | cat >&2; exit 0");
    let out = run_bounded(cmd, 10);
    assert!(!out.timed_out);
    assert!(out.status.success());
    assert!(out.stderr.len() >= 200_000);
}

#[test]
fn run_bounded_kills_child_that_ignores_sigterm() {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("trap '' TERM; sleep 120");
    let out = run_bounded(cmd, 2);
    assert!(out.timed_out, "must cap stubborn child");
}

#[test]
fn redact_strips_full_mhp_token() {
    let raw = "pat mhp_abc123XYZ-moretail leak";
    let red = redact_test_output(raw);
    assert!(!red.contains("mhp_abc123XYZ"));
    assert!(red.contains("[REDACTED_PAT]"));
}

#[test]
fn forward_verdict_admin_denial_without_echo() {
    assert_eq!(
        classify_forward_denial(
            true,
            false,
            false,
            "channel 0: open failed: administratively prohibited: open failed"
        ),
        ForwardDenialVerdict::Denied
    );
}

#[test]
fn forward_verdict_echo_roundtrip_means_channel_opened() {
    assert_eq!(
        classify_forward_denial(true, false, true, "any stderr"),
        ForwardDenialVerdict::ChannelOpened
    );
}

#[test]
fn forward_verdict_unrelated_stderr_inconclusive() {
    assert_eq!(
        classify_forward_denial(true, false, false, "Warning: Permanently added host key"),
        ForwardDenialVerdict::Inconclusive
    );
}

#[test]
fn forward_echo_probe_roundtrip_on_echo_target() {
    let (port, handle) = spawn_nonce_echo_target();
    thread::sleep(Duration::from_millis(50));
    let nonce = b"musehub-probe-roundtrip";
    assert!(forward_channel_echo_proven(port, nonce));
    let _ = handle.join();
}

#[test]
fn forward_echo_probe_rejects_accept_without_echo() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let acceptor = thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.write_all(b"not-the-nonce");
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    thread::sleep(Duration::from_millis(30));
    assert!(!forward_channel_echo_proven(port, b"nonce-bytes"));
    let _ = acceptor.join();
}

#[test]
#[cfg(unix)]
fn forward_echo_probe_detects_allowed_sshd_tunnel() {
    common::git_ssh_util::require_git_ssh_transport_prereqs();
    if !std::path::Path::new("/usr/sbin/sshd").exists() {
        eprintln!("skip permissive sshd positive: no /usr/sbin/sshd");
        return;
    }
    let tmp = tempfile::tempdir().expect("tmpdir");
    let host_key = tmp.path().join("hostkey");
    let user_key = tmp.path().join("userkey");
    assert!(Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            host_key.to_str().unwrap(),
            "-N",
            "",
            "-q",
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            user_key.to_str().unwrap(),
            "-N",
            "",
            "-q",
        ])
        .status()
        .unwrap()
        .success());
    let listener = TcpListener::bind("127.0.0.1:0").expect("pick");
    let ssh_port = listener.local_addr().unwrap().port();
    drop(listener);
    let pid_file = tmp.path().join("sshd.pid");
    let auth_keys = tmp.path().join("authorized_keys");
    std::fs::copy(user_key.with_extension("pub"), &auth_keys).expect("authorized_keys");
    let login_user = std::env::var("USER").unwrap_or_else(|_| "ubuntu".into());
    let cfg = format!(
        "Port {ssh_port}\nListenAddress 127.0.0.1\nHostKey {hk}\nPidFile {pid}\n\
         AllowTcpForwarding yes\nAuthorizedKeysFile {auth}\nPubkeyAuthentication yes\n\
         PasswordAuthentication no\nUsePAM no\nAllowUsers {login}\n",
        hk = host_key.display(),
        pid = pid_file.display(),
        auth = auth_keys.display(),
        login = login_user,
    );
    let cfg_path = tmp.path().join("sshd_config");
    std::fs::write(&cfg_path, cfg).expect("write cfg");
    let sshd_bin = if std::path::Path::new("/usr/sbin/sshd").exists() {
        "/usr/sbin/sshd"
    } else {
        "/usr/local/sbin/sshd"
    };
    assert!(
        Command::new("sudo")
            .args(["-n", sshd_bin, "-t", "-f", cfg_path.to_str().unwrap()])
            .status()
            .unwrap()
            .success(),
        "sshd_config must validate"
    );
    assert!(
        Command::new("sudo")
            .args(["-n", sshd_bin, "-f", cfg_path.to_str().unwrap()])
            .status()
            .unwrap()
            .success(),
        "sshd must start"
    );
    thread::sleep(Duration::from_millis(400));
    let (target_port, target_handle) = spawn_nonce_echo_target();
    thread::sleep(Duration::from_millis(50));
    let local_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_port = local_listener.local_addr().unwrap().port();
    drop(local_listener);
    let nonce = b"tunnel-positive-nonce";
    let mut forward = Command::new("ssh");
    forward.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "ConnectTimeout=5",
        "-i",
        user_key.to_str().unwrap(),
        "-p",
        &ssh_port.to_string(),
        "-N",
        "-L",
        &format!("127.0.0.1:{local_port}:127.0.0.1:{target_port}"),
        &format!("{login_user}@127.0.0.1"),
    ]);
    let mut child = forward.spawn().expect("ssh -N -L");
    thread::sleep(Duration::from_millis(500));
    if child.try_wait().ok().flatten().is_some() {
        eprintln!("skip permissive sshd tunnel: loopback ssh auth not available on host");
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            let _ = Command::new("sudo")
                .args(["-n", "kill", pid.trim()])
                .status();
        }
        let _ = target_handle.join();
        return;
    }
    let mut ok = false;
    for _ in 0..30 {
        if forward_channel_echo_proven(local_port, nonce) {
            ok = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    if let Ok(pid) = std::fs::read_to_string(&pid_file) {
        let _ = Command::new("sudo")
            .args(["-n", "kill", pid.trim()])
            .status();
    }
    let _ = target_handle.join();
    assert!(
        ok,
        "when permissive ssh -N -L stays up, echo roundtrip must succeed"
    );
}

#[tokio::test]
async fn product_musehub_git_serve_denies_forward_with_admin_stderr() {
    if std::env::var("MUSEHUB_TEST_DATABASE_URL").is_err() {
        eprintln!("skip product forward deny: no MUSEHUB_TEST_DATABASE_URL");
        return;
    }
    let f = git_ssh_fixture("fwdneg").await;
    assert_ssh_port_forward_denied(&f.priv_key, f.sshd.port);
}
