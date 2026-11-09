//! Process-boundary E2E for the reference adapter (no model).

use std::path::PathBuf;
use std::time::Duration;

use musehub_agent_adapter::{spawn_reference, AgentTask, RunOutcome, REFERENCE_ADAPTER};
use tempfile::TempDir;

#[tokio::test]
async fn reference_adapter_creates_commit() {
    let work = TempDir::new().expect("tempdir");
    std::process::Command::new("git")
        .args(["init", work.path().to_str().unwrap()])
        .status()
        .expect("git init");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_musehub-agent-reference"));
    let task = AgentTask {
        kind: "implementation".to_string(),
        message: "e2e reference".to_string(),
        branch: Some("agent/e2e".to_string()),
    };
    let run = spawn_reference(&task, work.path(), &binary)
        .await
        .expect("spawn");
    let outcome = musehub_agent_adapter::finish_run(run, Duration::from_secs(30), None, None)
        .await
        .expect("finish");
    match outcome {
        RunOutcome::Succeeded { commit, usage } => {
            assert!(commit.as_ref().is_some_and(|c| c.len() == 40));
            assert!(usage.budget_unknown);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    assert!(work.path().join("AGENT_TASK.txt").is_file());
}

#[tokio::test]
async fn reference_adapter_cancel_mid_run() {
    let work = TempDir::new().expect("tempdir");
    std::process::Command::new("git")
        .args(["init", work.path().to_str().unwrap()])
        .status()
        .expect("git init");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_musehub-agent-reference"));
    let task = AgentTask {
        kind: "implementation".to_string(),
        message: "slow".to_string(),
        branch: Some("agent/slow".to_string()),
    };
    let mut run = spawn_reference(&task, work.path(), &binary)
        .await
        .expect("spawn");
    run.request_cancel();
    let outcome = musehub_agent_adapter::finish_run(run, Duration::from_secs(30), None, None)
        .await
        .expect("finish");
    assert!(matches!(outcome, RunOutcome::Canceled { .. }));
    let _ = REFERENCE_ADAPTER;
}
