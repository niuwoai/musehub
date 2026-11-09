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
use serde::{Deserialize, Serialize};
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
pub struct CodexStatusResponse {
    pub adapter: String,
    pub live_binary_configured: bool,
    pub probe_allow_live: bool,
    /// True only after an explicit live run succeeded in this process (never set in CI).
    pub online_verified: bool,
    pub available_modes: Vec<&'static str>,
    pub note: &'static str,
}

/// Read-only model adapter entry: whether live Codex is configured vs CI sim-only probe.
pub async fn codex_status_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<CodexStatusResponse>, AppError> {
    let _principal = crate::require_session(&state.pool, &headers).await?;
    let live = codex_live_configured();
    let allow_live = std::env::var("MUSEHUB_CODEX_PROBE_ALLOW_LIVE")
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false);
    let mut modes = vec!["simulated_protocol"];
    if live {
        modes.push("live_binary_present");
        if allow_live {
            modes.push("live_probe_enabled");
        }
    }
    let note = if live && allow_live {
        "live probe permitted when explicitly enabled; credentials remain in user environment"
    } else if live {
        "live binary configured; HTTP probe stays sim unless MUSEHUB_CODEX_PROBE_ALLOW_LIVE=true"
    } else {
        "no live binary; use sim probe or set MUSEHUB_AGENT_CODEX_BIN"
    };
    Ok(Json(CodexStatusResponse {
        adapter: CODEX_ADAPTER.to_string(),
        live_binary_configured: live,
        probe_allow_live: allow_live,
        online_verified: false,
        available_modes: modes,
        note,
    }))
}

#[derive(Deserialize)]
pub struct CodexTaskRequest {
    pub message: String,
    pub budget_tokens: Option<u64>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Serialize)]
pub struct CodexTaskResponse {
    pub adapter: String,
    pub mode: &'static str,
    pub state: String,
    pub usage_tokens: Option<u64>,
    pub budget_tokens: Option<u64>,
    pub online_verified: bool,
    pub note: &'static str,
}

/// Headless Codex task: sim when live not allowed; live only with user binary + credentials in their environment.
pub async fn codex_task_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CodexTaskRequest>,
) -> Result<Json<CodexTaskResponse>, AppError> {
    let _principal = crate::require_session(&state.pool, &headers).await?;
    if body.message.trim().is_empty() {
        return Err(AppError::Validation("message required".to_string()));
    }
    let timeout = body.timeout_seconds.unwrap_or(30).clamp(5, 120);
    let work = TempDir::new().map_err(|e| AppError::Other(e.to_string()))?;
    let task = AgentTask {
        kind: "task".to_string(),
        message: body.message.trim().to_string(),
        branch: None,
    };
    let allow_live = std::env::var("MUSEHUB_CODEX_PROBE_ALLOW_LIVE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let (mode, run, online_verified) = if allow_live && codex_live_configured() {
        (
            "live_headless",
            spawn_codex_live(&task, work.path()).await?,
            true,
        )
    } else {
        if codex_live_configured() && !allow_live {
            return Err(AppError::Validation(
                "live codex configured but disabled; set MUSEHUB_CODEX_PROBE_ALLOW_LIVE=true"
                    .into(),
            ));
        }
        let bin = codex_sim_binary();
        (
            "simulated_headless",
            spawn_codex_sim(&task, work.path(), &bin)
                .await
                .map_err(|e| AppError::Other(e.to_string()))?,
            false,
        )
    };
    let outcome = finish_run(
        run,
        Duration::from_secs(timeout),
        body.budget_tokens,
        None,
        None,
    )
    .await
    .map_err(|e| AppError::Other(e.to_string()))?;
    let (state_name, usage) = match outcome {
        musehub_agent_adapter::RunOutcome::Succeeded { usage, .. } => {
            ("succeeded", Some(usage.tokens))
        }
        musehub_agent_adapter::RunOutcome::Failed { .. } => ("failed", None),
        musehub_agent_adapter::RunOutcome::Canceled { usage, .. } => {
            ("canceled", Some(usage.tokens))
        }
    };
    let note = if online_verified {
        "live run attempted; credentials were not read from the VM by MuseHub"
    } else {
        "simulated NDJSON; online model not verified"
    };
    Ok(Json(CodexTaskResponse {
        adapter: CODEX_ADAPTER.to_string(),
        mode,
        state: state_name.to_string(),
        usage_tokens: usage,
        budget_tokens: body.budget_tokens,
        online_verified,
        note,
    }))
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
