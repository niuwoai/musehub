//! Pipeline adapter: git archive bundle + optional local-registry OCI image (digest-pinned).
//! When `MUSEHUB_OCI_REGISTRY` is set, builds a minimal nginx-unprivileged image whose HTML body
//! is the pipeline `source_oid` (proves deploy matches built source).

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const ARTIFACT_OCI_REF: &str = "oci-image.ref";

fn main() {
    let source_oid = std::env::var("MUSEHUB_PIPELINE_SOURCE_OID").unwrap_or_default();
    let artifact_path = std::env::var("MUSEHUB_PIPELINE_ARTIFACT_PATH").unwrap_or_default();
    let bare = std::env::var("MUSEHUB_PIPELINE_BARE_GIT").unwrap_or_default();
    if source_oid.len() != 40 || artifact_path.is_empty() || bare.is_empty() {
        emit_error("invalid_env");
        std::process::exit(1);
    }
    println!(r#"{{"type":"started","source_oid":"{source_oid}"}}"#);
    let tree = Command::new("git")
        .args(["--git-dir", &bare, "archive", "--format=tar", &source_oid])
        .output();
    let output = match tree {
        Ok(o) if o.status.success() => o.stdout,
        _ => {
            emit_error("git_archive_failed");
            std::process::exit(1);
        }
    };
    let path = PathBuf::from(&artifact_path);
    let dir = match path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            emit_error("artifact_path_invalid");
            std::process::exit(1);
        }
    };
    if std::fs::create_dir_all(&dir).is_err() {
        emit_error("artifact_dir_failed");
        std::process::exit(1);
    }
    let mut file = match File::create(&path) {
        Ok(f) => f,
        Err(_) => {
            emit_error("artifact_create_failed");
            std::process::exit(1);
        }
    };
    if file.write_all(&output).is_err() {
        emit_error("artifact_write_failed");
        std::process::exit(1);
    }
    println!(
        r#"{{"type":"artifact","name":"bundle.tar","bytes":{}}}"#,
        output.len()
    );

    if let Ok(registry) = std::env::var("MUSEHUB_OCI_REGISTRY") {
        if registry.trim().is_empty() {
            println!(r#"{{"type":"completed","outcome":"succeeded"}}"#);
            return;
        }
        let run_id = std::env::var("MUSEHUB_PIPELINE_RUN_ID").unwrap_or_default();
        let repo_id = std::env::var("MUSEHUB_PIPELINE_REPO_ID").unwrap_or_default();
        match build_push_oci(&registry, &run_id, &repo_id, &source_oid, &dir) {
            Ok(oci_ref) => {
                let ref_path = dir.join(ARTIFACT_OCI_REF);
                if std::fs::write(&ref_path, &oci_ref).is_err() {
                    emit_error("oci_ref_write_failed");
                    std::process::exit(1);
                }
                println!(
                    r#"{{"type":"oci_image","name":"oci-image","oci_image_ref":"{oci_ref}"}}"#
                );
            }
            Err(code) => {
                emit_error(&code);
                std::process::exit(1);
            }
        }
    }

    println!(r#"{{"type":"completed","outcome":"succeeded"}}"#);
}

fn build_push_oci(
    registry: &str,
    run_id: &str,
    repo_id: &str,
    source_oid: &str,
    artifact_dir: &Path,
) -> Result<String, String> {
    if !docker_ok() {
        return Err("docker_unavailable".into());
    }
    let build_ctx = tempfile::tempdir().map_err(|_| "workspace_failed".to_string())?;
    let index = build_ctx.path().join("index.html");
    std::fs::write(&index, source_oid).map_err(|_| "oci_content_write_failed".to_string())?;
    let dockerfile = build_ctx.path().join("Dockerfile");
    let df = r#"FROM nginxinc/nginx-unprivileged:1.27-alpine
COPY index.html /usr/share/nginx/html/index.html
USER 101
EXPOSE 8080
"#;
    std::fs::write(&dockerfile, df).map_err(|_| "dockerfile_write_failed".to_string())?;

    let short = &source_oid[..12];
    let tag = format!("{registry}/musehub/pipeline-{repo_id}:{short}");
    let build = Command::new("docker")
        .args([
            "build",
            "-q",
            "-t",
            &tag,
            build_ctx.path().to_str().unwrap_or("."),
        ])
        .output()
        .map_err(|_| "docker_build_failed".to_string())?;
    if !build.status.success() {
        return Err("docker_build_failed".into());
    }

    let push = Command::new("docker")
        .args(["push", &tag])
        .output()
        .map_err(|_| "docker_push_failed".to_string())?;
    if !push.status.success() {
        return Err("docker_push_failed".into());
    }

    let inspect = Command::new("docker")
        .args([
            "image",
            "inspect",
            &tag,
            "--format",
            "{{index .RepoDigests 0}}",
        ])
        .output()
        .map_err(|_| "oci_digest_inspect_failed".to_string())?;
    if !inspect.status.success() {
        return Err("oci_digest_inspect_failed".into());
    }
    let repo_digest = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
    if !repo_digest.contains("@sha256:") {
        let pull_digest = String::from_utf8_lossy(&push.stdout);
        if let Some(line) = pull_digest.lines().find(|l| l.contains("@sha256:")) {
            let oci = line.trim().to_string();
            std::fs::write(artifact_dir.join(ARTIFACT_OCI_REF), &oci).ok();
            return Ok(oci);
        }
        return Err("oci_digest_unresolved".into());
    }
    let _ = std::env::var("MUSEHUB_PIPELINE_RUN_ID");
    let _ = run_id;
    Ok(repo_digest)
}

fn docker_ok() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn emit_error(code: &str) {
    println!(r#"{{"type":"error","code":"{code}"}}"#);
}
