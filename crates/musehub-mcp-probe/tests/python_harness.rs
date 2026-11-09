//! Boots the official Python `mcp` SDK client for matrix tests (independent of rmcp).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use serde::Deserialize;

pub const PYTHON_MCP_SDK_VERSION: &str = "2.3.0";
pub const PYTHON_CLIENT_IMPLEMENTATION: &str = "python-mcp-sdk";
pub const PYTHON_CLIENT_NAME: &str = "musehub-mcp-matrix-python";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PythonMatrixRun {
    pub client_implementation: String,
    pub client_sdk_version: String,
    pub client_name: String,
    pub transport: String,
    pub passed: bool,
    pub negotiated_protocol: String,
    pub tools_discovered: Vec<String>,
    pub checks: Vec<String>,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_root() -> PathBuf {
    manifest_dir().join("../..")
}

fn python_deps_dir() -> PathBuf {
    workspace_root().join("target/mcp-matrix-pydeps")
}

fn install_marker() -> PathBuf {
    python_deps_dir().join(format!(".mcp-{PYTHON_MCP_SDK_VERSION}-installed"))
}

pub fn python_deps_ready() -> bool {
    install_marker().is_file() && python_deps_dir().join("mcp").exists()
}

static INSTALL_PYTHON_DEPS: Once = Once::new();

/// Ensures Python `mcp==2.3.0` is importable from `target/mcp-matrix-pydeps`.
///
/// By default **does not** invoke pip during tests. Prepare deps with:
/// `./scripts/setup-mcp-matrix-python-deps.sh`
///
/// Set `MUSEHUB_MCP_MATRIX_ALLOW_PIP=1` to allow one-shot pip install (needs network).
pub fn ensure_python_mcp_sdk() {
    if python_deps_ready() {
        return;
    }
    let allow_pip = std::env::var("MUSEHUB_MCP_MATRIX_ALLOW_PIP")
        .ok()
        .as_deref()
        == Some("1");
    if !allow_pip {
        panic!(
            "Python mcp=={PYTHON_MCP_SDK_VERSION} is not prepared in {}. \
Run ./scripts/setup-mcp-matrix-python-deps.sh from the repo root \
(or set MUSEHUB_MCP_MATRIX_ALLOW_PIP=1 to install during the test — requires network).",
            python_deps_dir().display()
        );
    }
    INSTALL_PYTHON_DEPS.call_once(install_python_mcp_sdk_once);
    if !python_deps_ready() {
        install_python_mcp_sdk_once();
    }
}

fn install_python_mcp_sdk_once() {
    if install_marker().is_file() && python_deps_dir().join("mcp").exists() {
        return;
    }
    let deps = python_deps_dir();
    fs::create_dir_all(&deps).expect("create python deps dir");
    let requirements = manifest_dir().join("python/requirements.txt");
    assert!(requirements.is_file(), "missing {}", requirements.display());
    let status = Command::new("pip3")
        .args([
            "install",
            "--quiet",
            "--disable-pip-version-check",
            "--target",
        ])
        .arg(&deps)
        .arg("-r")
        .arg(&requirements)
        .status()
        .expect("pip3 install");
    assert!(
        status.success(),
        "pip3 install mcp=={PYTHON_MCP_SDK_VERSION} failed with {status}"
    );
    fs::write(install_marker(), b"ok").expect("write install marker");
}

fn runner_script() -> PathBuf {
    manifest_dir().join("python/mcp_matrix_runner.py")
}

fn run_python(args: &[&str]) -> PythonMatrixRun {
    ensure_python_mcp_sdk();
    let script = runner_script();
    assert!(script.is_file(), "missing {}", script.display());

    let mut command = Command::new("python3");
    command
        .env(
            "PYTHONPATH",
            prepend_python_path(python_deps_dir().as_path()),
        )
        .arg(&script);
    for arg in args {
        command.arg(arg);
    }

    let output = command.output().expect("run python matrix runner");
    assert!(
        output.status.success(),
        "python runner failed: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse python matrix JSON")
}

fn prepend_python_path(deps: &Path) -> String {
    match std::env::var("PYTHONPATH") {
        Ok(existing) if !existing.is_empty() => {
            format!("{}:{}", deps.display(), existing)
        }
        _ => deps.display().to_string(),
    }
}

pub fn python_stdio_matrix(stdio_bin: &str) -> PythonMatrixRun {
    run_python(&["--mode", "stdio", "--stdio-bin", stdio_bin])
}

pub fn python_http_matrix(url: &str, bearer: &str) -> PythonMatrixRun {
    run_python(&["--mode", "http", "--http-url", url, "--bearer", bearer])
}

pub fn python_http_bearer_reject(url: &str) {
    ensure_python_mcp_sdk();
    let script = runner_script();
    let mut command = Command::new("python3");
    command
        .env(
            "PYTHONPATH",
            prepend_python_path(python_deps_dir().as_path()),
        )
        .arg(&script)
        .args(["--mode", "http-bearer-reject", "--http-url", url]);
    let output = command.output().expect("python bearer reject");
    assert!(
        output.status.success(),
        "python bearer reject failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["passed"], true);
}
