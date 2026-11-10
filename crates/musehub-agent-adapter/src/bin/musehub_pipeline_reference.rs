//! Controlled pipeline reference adapter: checkout ODB at fixed oid, write artifact tarball.
//! No shell — fixed argv and env only.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

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
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
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
    println!(r#"{{"type":"completed","outcome":"succeeded"}}"#);
}

fn emit_error(code: &str) {
    println!(r#"{{"type":"error","code":"{code}"}}"#);
}
