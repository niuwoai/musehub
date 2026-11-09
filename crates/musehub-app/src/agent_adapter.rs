//! AgentRun adapter selection for the product worker mainline (reference / codex / codex-sim).

use std::path::{Path, PathBuf};

use musehub_agent_adapter::{
    spawn_codex, spawn_codex_sim, spawn_reference, AdapterRun, AgentTask, CODEX_ADAPTER,
    CODEX_SIM_ADAPTER, REFERENCE_ADAPTER,
};

use crate::codex::codex_live_configured;

/// Resolved adapter stored on `agent_runs.adapter_kind` when a run is queued.
pub fn adapter_kind_for_request(requested: Option<&str>) -> &'static str {
    if std::env::var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_some()
    {
        return CODEX_ADAPTER;
    }
    match requested.unwrap_or(REFERENCE_ADAPTER) {
        "codex" => CODEX_ADAPTER,
        "codex-sim" => CODEX_SIM_ADAPTER,
        _ => REFERENCE_ADAPTER,
    }
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

/// Spawn the configured adapter process for this run (fixture wins for CI).
pub async fn spawn_agent_adapter(
    adapter_kind: &str,
    task: &AgentTask,
    work_dir: &Path,
    reference_bin: &Path,
) -> Result<AdapterRun, &'static str> {
    if let Ok(fixture) = std::env::var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN") {
        let path = Path::new(fixture.trim());
        return spawn_codex_sim(task, work_dir, path)
            .await
            .map_err(|_| "adapter_spawn_failed");
    }
    if adapter_kind == CODEX_SIM_ADAPTER {
        let sim = codex_sim_binary();
        if !sim.is_file() {
            return Err("codex_sim_not_configured");
        }
        return spawn_codex_sim(task, work_dir, &sim)
            .await
            .map_err(|_| "adapter_spawn_failed");
    }
    if adapter_kind == CODEX_ADAPTER {
        if !codex_live_configured() {
            return Err("codex_not_configured");
        }
        return spawn_codex(task, work_dir)
            .await
            .map_err(|_| "adapter_spawn_failed");
    }
    spawn_reference(task, work_dir, reference_bin)
        .await
        .map_err(|_| "adapter_spawn_failed")
}
