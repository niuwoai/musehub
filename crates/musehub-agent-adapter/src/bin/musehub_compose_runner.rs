//! Isolated `docker compose` deploy runner (no host socket bind in compose file).
//! Requires `MUSEHUB_OCI_IMAGE_REF` (digest-pinned); control plane generates compose YAML.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

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

fn valid_oci_digest_ref(image: &str) -> bool {
    let image = image.trim();
    let Some((_, digest)) = image.split_once("@sha256:") else {
        return false;
    };
    digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit())
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
    let oci_ref = match std::env::var("MUSEHUB_OCI_IMAGE_REF")
        .or_else(|_| std::env::var("MUSEHUB_NGINX_IMAGE_DIGEST"))
    {
        Ok(v) if valid_oci_digest_ref(&v) => v,
        Ok(_) => {
            emit(r#"{"type":"error","code":"oci_image_ref_invalid"}"#);
            std::process::exit(1);
        }
        Err(_) => {
            emit(r#"{"type":"error","code":"oci_image_ref_missing"}"#);
            std::process::exit(1);
        }
    };
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
    image: {oci_ref}
    ports:
      - "127.0.0.1:{publish_port}:8080"
    read_only: true
    tmpfs:
      - /var/cache/nginx:size=8m
      - /var/run:size=1m
      - /tmp:size=1m
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
        r#"{{"type":"started","target":"{target}","project":"{project}","oci_image_ref":"{oci_ref}"}}"#
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
        ])
        .current_dir(work.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let up_ok = up.as_ref().map(|o| o.status.success()).unwrap_or(false);
    if !up_ok {
        rollback(&compose_path, &project);
        if let Ok(o) = up {
            let detail = String::from_utf8_lossy(&o.stderr)
                .lines()
                .take(8)
                .collect::<Vec<_>>()
                .join(" | ");
            emit(&format!(
                r#"{{"type":"error","code":"compose_up_failed","detail":{}}}"#,
                serde_json::to_string(&detail).unwrap_or_else(|_| "\"\"".into())
            ));
        } else {
            emit(r#"{"type":"error","code":"compose_up_failed"}"#);
        }
        std::process::exit(1);
    }

    let image_id = Command::new("docker")
        .args(["compose", "-p", &project, "images", "-q"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let mut applied_digest = Command::new("docker")
        .args([
            "image",
            "inspect",
            &oci_ref,
            "--format",
            "{{index .RepoDigests 0}}",
        ])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            s.trim()
                .split('@')
                .nth(1)
                .filter(|d| d.starts_with("sha256:") && d.len() == 71)
                .map(String::from)
        })
        .unwrap_or_default();
    if applied_digest.is_empty() {
        applied_digest = image_id
            .strip_prefix("sha256:")
            .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|s| format!("sha256:{s}"))
            .unwrap_or_default();
    }
    if applied_digest.is_empty() {
        applied_digest = Command::new("docker")
            .args(["inspect", &image_id, "--format", "{{index .RepoDigests 0}}"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.trim()
                    .split('@')
                    .nth(1)
                    .filter(|d| d.starts_with("sha256:"))
                    .map(String::from)
            })
            .unwrap_or_default();
    }

    let requested_digest = oci_ref.split('@').nth(1).unwrap_or("").to_string();
    if applied_digest.is_empty() {
        rollback(&compose_path, &project);
        emit(r#"{"type":"error","code":"oci_digest_unresolved"}"#);
        std::process::exit(1);
    }
    if !requested_digest.is_empty() && requested_digest != applied_digest {
        rollback(&compose_path, &project);
        emit(&format!(
            r#"{{"type":"error","code":"oci_digest_mismatch","requested":"{requested_digest}","applied":"{applied_digest}"}}"#
        ));
        std::process::exit(1);
    }

    let expected_commit = std::env::var("MUSEHUB_RELEASE_COMMIT_OID").unwrap_or_default();
    let mut healthy = false;
    let mut verified_source_oid = String::new();
    for _ in 0..40 {
        if let Some(body) = probe_http_body(&target) {
            if !expected_commit.is_empty() && body.trim() != expected_commit {
                rollback(&compose_path, &project);
                emit(&format!(
                    r#"{{"type":"error","code":"source_oid_mismatch","expected":"{expected_commit}","body":{}}}"#,
                    serde_json::to_string(body.trim()).unwrap_or_else(|_| "\"\"".into())
                ));
                std::process::exit(1);
            }
            verified_source_oid = body.trim().to_string();
            healthy = true;
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    if healthy {
        emit(&format!(
            r#"{{"type":"health","status":"healthy","manifest_sha256":"{manifest}","adapter":"compose-runner","container_image_id":"{image_id}","oci_image_ref":"{oci_ref}","applied_oci_digest":"{applied_digest}","verified_source_oid":"{verified_source_oid}"}}"#
        ));
        emit(r#"{"type":"completed","outcome":"succeeded"}"#);
        if !leave_stack {
            rollback(&compose_path, &project);
        }
        std::process::exit(0);
    }

    rollback(&compose_path, &project);
    emit(&format!(
        r#"{{"type":"health","status":"unhealthy","adapter":"compose-runner","container_image_id":"{image_id}","oci_image_ref":"{oci_ref}"}}"#
    ));
    emit(r#"{"type":"completed","outcome":"failed"}"#);
    std::process::exit(1);
}

fn probe_http_body(target: &str) -> Option<String> {
    if target.starts_with("mock://") {
        return (target == "mock://healthy").then(|| "mock".into());
    }
    let out = Command::new("curl")
        .args(["-sf", "--max-time", "8", target])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn uuid_simple() -> String {
    use std::sync::atomic::{AtomicU64, Ordering as O};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, O::Relaxed);
    format!("{n}")
}
