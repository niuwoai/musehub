//! Protocol-only Codex sim E2E (no live model).

use std::path::PathBuf;
use std::time::Duration;

use musehub_agent_adapter::{spawn_codex_sim, AgentTask, RunOutcome, CODEX_ADAPTER};
use tempfile::TempDir;

#[tokio::test]
async fn codex_sim_emits_completed_without_commit() {
    let work = TempDir::new().expect("tempdir");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_musehub-agent-codex-sim"));
    let task = AgentTask {
        kind: "probe".to_string(),
        message: "codex sim".to_string(),
        branch: None,
    };
    let run = spawn_codex_sim(&task, work.path(), &binary)
        .await
        .expect("spawn");
    let outcome = musehub_agent_adapter::finish_run(run, Duration::from_secs(15), None, None, None)
        .await
        .expect("finish");
    match outcome {
        RunOutcome::Succeeded { commit, usage } => {
            assert!(commit.is_none());
            assert_eq!(usage.tokens, 1);
        }
        other => panic!("unexpected {:?} for {}", other, CODEX_ADAPTER),
    }
}
