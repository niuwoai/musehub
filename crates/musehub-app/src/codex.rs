#![allow(dead_code)]
//! Optional Codex CLI adapter wiring (requires user-provided binary + credentials).
//!
//! **Live model tasks are not verified in CI.** This module only exposes configuration helpers.

use std::path::Path;

use musehub_agent_adapter::{spawn_codex, AgentTask, CODEX_ADAPTER};

use crate::error::AppError;

/// Returns whether live Codex invocation is configured (binary exists on disk).
pub fn codex_live_configured() -> bool {
    std::env::var("MUSEHUB_AGENT_CODEX_BIN")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .is_some_and(|path| Path::new(path.trim()).is_file())
}

/// Spawns Codex when configured; otherwise returns a clear error (no silent fallback).
pub async fn spawn_codex_live(
    task: &AgentTask,
    work_dir: &Path,
) -> Result<musehub_agent_adapter::AdapterRun, AppError> {
    if !codex_live_configured() {
        return Err(AppError::Other(format!(
            "{CODEX_ADAPTER} is not configured: set MUSEHUB_AGENT_CODEX_BIN to the codex executable and provide API credentials in the user environment"
        )));
    }
    spawn_codex(task, work_dir)
        .await
        .map_err(|error| AppError::Other(error.to_string()))
}
