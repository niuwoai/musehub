//! Isolated `docker compose` deploy runner (no host socket bind in compose file).
//! Emits NDJSON like compose-mock; exits 2 when Docker/Compose is unavailable.

use std::io::Write;
use std::process::{Command, Stdio};
const NGINX_DIGEST: &str =
    "nginx:1.27.3@sha256:bc5e5e2cfc94ba04666678eae575013367240a9e4c3e3f3d0ed3860f2e914a3";

fn emit(line: &str) {
    let _ = writeln!(std::io::stdout(), "{line}");
}

fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn main() {
    let target = std::env::var("MUSEHUB_DEPLOY_HEALTH_TARGET")
        .unwrap_or_else(|_| "http://127.0.0.1:18080/".into());
    let manifest = std::env::var("MUSEHUB_RELEASE_MANIFEST_SHA256").unwrap_or_default();
    let project = std::env::var("MUSEHUB_COMPOSE_PROJECT")
        .unwrap_or_else(|_| format!("musehub-deploy-{}", uuid_simple()));
    let publish_port =
        std::env::var("MUSEHUB_COMPOSE_PUBLISH_PORT").unwrap_or_else(|_| "18080".into());

    if !docker_available() {
        emit(r#"{"type":"error","code":"docker_unavailable"}"#);
        std::process::exit(2);
    }

    let work = match tempfile::tempdir() {
        Ok(d) => d,
        Err(_) => {
            emit(r#"{"type":"error","code":"workspace_failed"}"#);
            std::process::exit(1);
        }
    };

    let compose = format!(
        r#"services:
  web:
    image: {NGINX_DIGEST}
    ports:
      - "127.0.0.1:{publish_port}:80"
    read_only: true
    tmpfs:
      - /var/cache/nginx:size=8m
      - /var/run:size=1m
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL
    pids_limit: 128
    mem_limit: 128m
"#
    );
    let compose_path = work.path().join("compose.yml");
    if std::fs::write(&compose_path, compose).is_err() {
        emit(r#"{"type":"error","code":"compose_write_failed"}"#);
        std::process::exit(1);
    }

    emit(&format!(
        r#"{{"type":"started","target":"{target}","project":"{project}"}}"#
    ));

    let up = Command::new("docker")
        .args([
            "compose",
            "-f",
            compose_path.to_str().unwrap_or("compose.yml"),
            "-p",
            &project,
            "up",
            "-d",
            "--pull",
            "always",
            "--wait",
        ])
        .current_dir(work.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status();

    let rollback = || {
        let _ = Command::new("docker")
            .args([
                "compose",
                "-f",
                compose_path.to_str().unwrap_or("compose.yml"),
                "-p",
                &project,
                "down",
                "-v",
                "--remove-orphans",
            ])
            .current_dir(work.path())
            .status();
    };

    if !up.map(|s| s.success()).unwrap_or(false) {
        rollback();
        emit(r#"{"type":"error","code":"compose_up_failed"}"#);
        std::process::exit(1);
    }

    let healthy = probe_http(&target);
    if healthy {
        emit(&format!(
            r#"{{"type":"health","status":"healthy","manifest_sha256":"{manifest}","adapter":"compose-runner"}}"#
        ));
        emit(r#"{"type":"completed","outcome":"succeeded"}"#);
        // leave stack up briefly for external probes; tear down on success path for CI hygiene
        rollback();
        std::process::exit(0);
    }

    rollback();
    emit(r#"{"type":"health","status":"unhealthy","adapter":"compose-runner"}"#);
    emit(r#"{"type":"completed","outcome":"failed"}"#);
    std::process::exit(1);
}

fn probe_http(target: &str) -> bool {
    if target.starts_with("mock://") {
        return target == "mock://healthy";
    }
    Command::new("curl")
        .args(["-sf", "--max-time", "8", target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn uuid_simple() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    format!("{n}")
}
