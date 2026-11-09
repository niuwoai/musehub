//! Isolated `docker compose` deploy runner (no host socket bind in compose file).
//! Control plane supplies health target only; compose YAML is generated here.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_NGINX_DIGEST: &str =
    "nginx:1.27.3@sha256:bc5e5e2cfc94ba04666678eae575013367240a9e4c3e3d0ed3860f2e914a3";

static ROLLBACK_DONE: AtomicBool = AtomicBool::new(false);

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

fn rollback(compose_path: &std::path::Path, project: &str) {
    if ROLLBACK_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = Command::new("docker")
        .args([
            "compose",
            "-f",
            compose_path.to_str().unwrap_or("compose.yml"),
            "-p",
            project,
            "down",
            "-v",
            "--remove-orphans",
        ])
        .status();
}

fn main() {
    let target = std::env::var("MUSEHUB_DEPLOY_HEALTH_TARGET")
        .unwrap_or_else(|_| "http://127.0.0.1:18080/".into());
    let manifest = std::env::var("MUSEHUB_RELEASE_MANIFEST_SHA256").unwrap_or_default();
    let project = std::env::var("MUSEHUB_COMPOSE_PROJECT")
        .unwrap_or_else(|_| format!("musehub-deploy-{}", uuid_simple()));
    let publish_port =
        std::env::var("MUSEHUB_COMPOSE_PUBLISH_PORT").unwrap_or_else(|_| "18080".into());
    let nginx_image = std::env::var("MUSEHUB_NGINX_IMAGE_DIGEST")
        .unwrap_or_else(|_| DEFAULT_NGINX_IMAGE.to_string());
    let leave_stack = std::env::var("MUSEHUB_COMPOSE_LEAVE_STACK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

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
    image: {nginx_image}
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
        r#"{{"type":"started","target":"{target}","project":"{project}","image":"{nginx_image}"}}"#
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

    if !up.map(|s| s.success()).unwrap_or(false) {
        rollback(&compose_path, &project);
        emit(r#"{"type":"error","code":"compose_up_failed"}"#);
        std::process::exit(1);
    }

    let image_id = Command::new("docker")
        .args(["compose", "-p", &project, "images", "-q"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let healthy = probe_http(&target);
    if healthy {
        emit(&format!(
            r#"{{"type":"health","status":"healthy","manifest_sha256":"{manifest}","adapter":"compose-runner","container_image_id":"{image_id}","requested_image":"{nginx_image}"}}"#
        ));
        emit(r#"{"type":"completed","outcome":"succeeded"}"#);
        if !leave_stack {
            rollback(&compose_path, &project);
        }
        std::process::exit(0);
    }

    rollback(&compose_path, &project);
    emit(&format!(
        r#"{{"type":"health","status":"unhealthy","adapter":"compose-runner","container_image_id":"{image_id}"}}"#
    ));
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
