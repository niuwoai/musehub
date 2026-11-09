//! Compose deploy adapter selection (mock vs isolated runner).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeAdapterMode {
    Mock,
    Runner,
    Auto,
}

pub fn compose_mode_from_env() -> ComposeAdapterMode {
    let raw = std::env::var("MUSEHUB_COMPOSE_ADAPTER_MODE")
        .or_else(|_| std::env::var("MUSEHUB_COMPOSE_MODE"))
        .unwrap_or_else(|_| "mock".into())
        .to_ascii_lowercase();
    match raw.as_str() {
        "runner" => ComposeAdapterMode::Runner,
        "auto" => ComposeAdapterMode::Auto,
        _ => ComposeAdapterMode::Mock,
    }
}

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn resolve_compose_adapter(
    mock_bin: &Path,
    runner_bin: &Path,
    require_product_oci: bool,
) -> Result<(PathBuf, &'static str), String> {
    let mode = compose_mode_from_env();
    if require_product_oci && mode == ComposeAdapterMode::Mock {
        return Err("product OCI path forbids compose-mock adapter".into());
    }
    Ok(match mode {
        ComposeAdapterMode::Mock => (mock_bin.to_path_buf(), "compose-mock"),
        ComposeAdapterMode::Runner => (runner_bin.to_path_buf(), "compose-runner"),
        ComposeAdapterMode::Auto => {
            if require_product_oci || docker_available() {
                if !docker_available() {
                    return Err("compose-runner required but docker unavailable".into());
                }
                (runner_bin.to_path_buf(), "compose-runner")
            } else {
                (mock_bin.to_path_buf(), "compose-mock")
            }
        }
    })
}
