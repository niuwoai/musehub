use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub struct TestGitSshd {
    pub port: u16,
    child: Child,
    run_dir: PathBuf,
}

impl TestGitSshd {
    pub fn start(
        git_root: &std::path::Path,
        database_url: &str,
        gateway_bin: &std::path::Path,
    ) -> Self {
        let port = pick_port();
        let run_dir = tempfile::tempdir().expect("sshd tempdir").into_path();
        let exe = gateway_bin.to_path_buf();
        let child = Command::new(&exe)
            .args(["serve", &port.to_string()])
            .env("DATABASE_URL", database_url)
            .env("MUSEHUB_GIT_ROOT", git_root)
            .env("MUSEHUB_GIT_SSH_RUN_DIR", &run_dir)
            .env("MUSEHUB_GIT_SSH_GATEWAY_BIN", gateway_bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn musehub-git-ssh serve");
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
        }
        Self {
            port,
            child,
            run_dir,
        }
    }
}

impl Drop for TestGitSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.run_dir);
    }
}

fn pick_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}
