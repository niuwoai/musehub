//! Optional Codex CLI adapter wiring (requires user-provided binary + credentials).
//!
//! **Live model tasks are not verified in CI.** HTTP probe uses the sim binary unless live is configured.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use musehub_agent_adapter::{finish_run, spawn_codex, spawn_codex_sim, AgentTask, CODEX_ADAPTER};
use serde::Serialize;
use tempfile::TempDir;

use crate::error::AppError;
use crate::AppState;

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

fn codex_sim_binary() -> PathBuf {
    std::env::var("MUSEHUB_AGENT_CODEX_SIM_BIN")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-agent-codex-sim").map(PathBuf::from))
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-agent-codex-sim")
        })
}

#[derive(Serialize)]
pub struct CodexProbeResponse {
    pub adapter: String,
    pub mode: &'static str,
    pub state: String,
    pub note: &'static str,
}

/// Runs a one-shot adapter probe (sim by default; live only when configured).
pub async fn codex_probe_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<CodexProbeResponse>, AppError> {
    let _principal = crate::require_session(&state.pool, &headers).await?;
    let work = TempDir::new().map_err(|error| AppError::Other(error.to_string()))?;
    let task = AgentTask {
        kind: "probe".to_string(),
        message: "http probe".to_string(),
        branch: None,
    };
    let allow_live = std::env::var("MUSEHUB_CODEX_PROBE_ALLOW_LIVE")
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false);
    let (mode, run) = if allow_live && codex_live_configured() {
        (
            "live_attempt_no_ci_evidence",
            spawn_codex_live(&task, work.path()).await?,
        )
    } else {
        if codex_live_configured() && !allow_live {
            return Err(AppError::Validation(
                "live codex binary is configured but probe is sim-only; set MUSEHUB_CODEX_PROBE_ALLOW_LIVE=true to attempt live".to_string(),
            ));
        }
        let bin = codex_sim_binary();
        (
            "simulated_protocol",
            spawn_codex_sim(&task, work.path(), &bin)
                .await
                .map_err(|error| AppError::Other(error.to_string()))?,
        )
    };
    let outcome = finish_run(run, Duration::from_secs(30), None, None, None)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    let state_name = match outcome {
        musehub_agent_adapter::RunOutcome::Succeeded { .. } => "succeeded",
        musehub_agent_adapter::RunOutcome::Failed { .. } => "failed",
        musehub_agent_adapter::RunOutcome::Canceled { .. } => "canceled",
    };
    let note = if mode.starts_with("live") {
        "explicit live attempt; no online model evidence recorded"
    } else {
        "simulated NDJSON only; does not execute live codex"
    };
    Ok(Json(CodexProbeResponse {
        adapter: CODEX_ADAPTER.to_string(),
        mode,
        state: state_name.to_string(),
        note,
    }))
}
