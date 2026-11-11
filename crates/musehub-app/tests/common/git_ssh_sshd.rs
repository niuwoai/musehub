use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub struct TestGitSshd {
    pub port: u16,
    pub run_dir: PathBuf,
    child: Child,
}

impl TestGitSshd {
    pub fn start(
        git_root: &std::path::Path,
        database_url: &str,
        gateway_bin: &std::path::Path,
    ) -> Self {
        ensure_git_os_user();
        let port = pick_port();
        let run_dir = prepare_root_sshd_run_dir(port);
        let bin_path = install_root_owned_git_ssh_bin(gateway_bin);
        let mut sudo = Command::new("sudo");
        sudo.arg("env").arg("-i");
        sudo.arg(format!(
            "HOME={}",
            std::env::var("HOME").unwrap_or_else(|_| "/root".into())
        ));
        sudo.arg(format!(
            "PATH={}",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        ));
        sudo.arg(format!("DATABASE_URL={database_url}"));
        sudo.arg(format!("MUSEHUB_GIT_ROOT={}", git_root.display()));
        sudo.arg(format!("MUSEHUB_GIT_SSH_RUN_DIR={}", run_dir.display()));
        sudo.arg("MUSEHUB_GIT_STORAGE_GROUP=musehub-git");
        sudo.arg("MUSEHUB_GIT_SSH_RUNTIME_GROUP=git");
        sudo.arg("MUSEHUB_GIT_SSH_SUDO_SSHD=1");
        sudo.arg(if std::env::var("MUSEHUB_GIT_SSH_TEST_SSHD_LOG").is_ok() {
            "MUSEHUB_GIT_SSH_SSHD_LOG=VERBOSE"
        } else {
            "MUSEHUB_GIT_SSH_SSHD_LOG=ERROR"
        });
        if let Ok(marker) = std::env::var("MUSEHUB_GIT_SSH_PACKAGING_TEST_MARKER") {
            if !marker.is_empty() {
                sudo.arg(format!("MUSEHUB_GIT_SSH_PACKAGING_TEST_MARKER={marker}"));
            }
        }
        sudo.arg(&bin_path);
        sudo.args(["serve", &port.to_string()]);
        let mut child = sudo
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(if std::env::var("MUSEHUB_GIT_SSH_TEST_SSHD_LOG").is_ok() {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .spawn()
            .expect("spawn musehub-git-ssh serve (sudo)");
        let mut ready = false;
        for _ in 0..80 {
            std::thread::sleep(Duration::from_millis(100));
            if let Ok(Some(status)) = child.try_wait() {
                panic!("musehub-git-ssh serve exited before binding port: {status}");
            }
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                ready = true;
                break;
            }
        }
        assert!(
            ready,
            "git sshd did not accept connections on 127.0.0.1:{port} — required for CI"
        );
        std::thread::sleep(Duration::from_millis(200));
        let runtime = run_dir.join("runtime.env");
        let runtime_ok = Command::new("sudo")
            .args(["test", "-f", runtime.to_str().expect("runtime")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !runtime_ok {
            let ls = Command::new("sudo")
                .args(["ls", "-la", run_dir.to_str().unwrap_or("")])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            panic!(
                "serve must write runtime.env under {} (ls:\n{ls})",
                run_dir.display()
            );
        }
        Self {
            port,
            run_dir,
            child,
        }
    }
}

impl Drop for TestGitSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let rd = self.run_dir.to_string_lossy();
        let _ = Command::new("sudo")
            .args(["rm", "-rf", rd.as_ref()])
            .status();
    }
}

fn pick_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

pub fn ensure_git_storage_group() {
    if !Command::new("getent")
        .args(["group", "musehub-git"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        let _ = Command::new("sudo")
            .args(["groupadd", "-r", "musehub-git"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let whoami = std::env::var("USER").unwrap_or_else(|_| "ubuntu".into());
    for account in ["git", whoami.as_str()] {
        let _ = Command::new("sudo")
            .args(["usermod", "-aG", "musehub-git", account])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    std::env::set_var("MUSEHUB_GIT_STORAGE_GROUP", "musehub-git");
    std::env::set_var("MUSEHUB_GIT_STORAGE_SUDO_FALLBACK", "1");
}

fn ensure_git_os_user() {
    ensure_git_storage_group();
    let exists = Command::new("id")
        .arg("git")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !exists {
        let args = ["useradd", "-r", "-M", "-s", "/bin/false", "git"];
        let created = Command::new("sudo")
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            || Command::new("useradd")
                .args(args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        assert!(
            created,
            "git system user is required for Git-over-SSH acceptance tests"
        );
    }
    // OpenSSH rejects pubkey auth for shadow-locked service accounts (`*` password field).
    let _ = Command::new("sudo")
        .args(["passwd", "-d", "git"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let login_shell = "/usr/local/lib/musehub/musehub-git-login.sh";
    if std::path::Path::new(login_shell).exists() {
        let _ = Command::new("sudo")
            .args(["usermod", "-s", login_shell, "git"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// OpenSSH requires root-owned, non-world-writable parents for `AuthorizedKeysCommand` (not under `/tmp`).
fn prepare_root_sshd_run_dir(port: u16) -> PathBuf {
    let run_dir = PathBuf::from(format!("/var/lib/musehub-ssh-test-{port}"));
    let rd = run_dir.to_string_lossy();
    let script = format!(
        "rm -rf {rd} && install -d -m 0750 -o root -g git {rd} && chown root:git {rd} && chmod 0750 {rd}"
    );
    let status = Command::new("sudo")
        .args(["bash", "-c", &script])
        .status()
        .expect("sudo install git-ssh run dir");
    assert!(
        status.success(),
        "root-owned git-ssh run dir required for OpenSSH AKC path checks: {}",
        run_dir.display()
    );
    run_dir
}

pub fn ssh_clone_user() -> &'static str {
    "git"
}

pub fn install_root_owned_git_ssh_bin(gateway_bin: &std::path::Path) -> std::path::PathBuf {
    let dest = std::path::PathBuf::from("/usr/local/bin/musehub-git-ssh");
    let _ = Command::new("sudo")
        .args(["rm", "-f", dest.to_str().expect("dest")])
        .status();
    let status = Command::new("sudo")
        .args([
            "install",
            "-m",
            "0755",
            gateway_bin.to_str().expect("bin"),
            dest.to_str().expect("dest"),
        ])
        .status()
        .expect("sudo install musehub-git-ssh");
    assert!(
        status.success(),
        "sudo install musehub-git-ssh to /usr/local/bin required for AKC ownership"
    );
    dest
}
