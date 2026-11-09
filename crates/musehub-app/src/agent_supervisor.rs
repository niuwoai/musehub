use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use musehub_agent_adapter::{spawn_reference, AgentTask, RunOutcome, UsageTotals};
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::error::AppError;

pub fn new_supervisor() -> Arc<AgentSupervisor> {
    Arc::new(AgentSupervisor::default())
}

#[derive(Default)]
pub struct AgentSupervisor {
    inner: Mutex<HashMap<Uuid, KillGuard>>,
}

struct KillGuard {
    pid: u32,
}

impl AgentSupervisor {
    pub async fn register(&self, run_id: Uuid, pid: u32) {
        self.inner.lock().await.insert(run_id, KillGuard { pid });
    }

    pub async fn cancel(&self, run_id: Uuid) -> bool {
        let pid = self
            .inner
            .lock()
            .await
            .remove(&run_id)
            .map(|guard| guard.pid);
        if let Some(pid) = pid {
            kill_process_group(pid);
            return true;
        }
        false
    }
}

#[cfg(unix)]
pub fn kill_process_group(pid: u32) {
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub fn kill_process_group(_pid: u32) {}

#[allow(clippy::too_many_arguments)]
pub async fn run_reference_job(
    pool: PgPool,
    supervisor: Arc<AgentSupervisor>,
    run_id: Uuid,
    repo_id: Uuid,
    _storage_key: Uuid,
    git_root: PathBuf,
    reference_bin: PathBuf,
    branch: String,
    message: String,
    deadline: Duration,
    budget_tokens: Option<u64>,
) {
    let _ = sqlx::query(
        "UPDATE agent_runs SET state = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(run_id)
    .execute(&pool)
    .await;

    let storage_key =
        match sqlx::query_as::<_, (Uuid,)>("SELECT storage_key FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_optional(&pool)
            .await
        {
            Ok(Some((key,))) => key,
            _ => {
                let _ = mark_failed(&pool, run_id, "repo_metadata_failed").await;
                return;
            }
        };
    let work_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(_) => {
            let _ = mark_failed(&pool, run_id, "workspace_failed").await;
            return;
        }
    };
    let bare = git_root.join(format!("{storage_key}.git"));
    if !std::process::Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap_or("."),
            work_dir.path().to_str().unwrap_or("."),
        ])
        .status()
        .is_ok_and(|s| s.success())
    {
        let _ = mark_failed(&pool, run_id, "git_clone_failed").await;
        return;
    }
    let _ = std::process::Command::new("git")
        .current_dir(work_dir.path())
        .args(["checkout", "-B", &branch, "main"])
        .status();

    let task = AgentTask {
        kind: "implementation".to_string(),
        message,
        branch: Some(branch.clone()),
    };
    let run = match spawn_reference(&task, work_dir.path(), &reference_bin).await {
        Ok(run) => run,
        Err(_) => {
            let _ = mark_failed(&pool, run_id, "adapter_spawn_failed").await;
            return;
        }
    };
    if let Some(pid) = run.child_id() {
        supervisor.register(run_id, pid).await;
    }
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let pool_poll = pool.clone();
    let poll = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let row = sqlx::query_as::<_, (String,)>("SELECT state FROM agent_runs WHERE id = $1")
                .bind(run_id)
                .fetch_optional(&pool_poll)
                .await;
            let Ok(Some((state,))) = row else {
                continue;
            };
            if state == "cancel_requested" {
                let _ = cancel_tx.send(true);
                break;
            }
            if state != "queued" && state != "running" && state != "cancel_requested" {
                break;
            }
        }
    });
    let outcome = match musehub_agent_adapter::finish_run(
        run,
        deadline,
        budget_tokens,
        None,
        Some(cancel_rx),
    )
    .await
    {
        Ok(outcome) => normalize_cancel_outcome(&pool, run_id, outcome).await,
        Err(_) => {
            let _ = mark_failed(&pool, run_id, "adapter_failed").await;
            supervisor.inner.lock().await.remove(&run_id);
            poll.abort();
            return;
        }
    };
    poll.abort();
    supervisor.inner.lock().await.remove(&run_id);

    match outcome {
        RunOutcome::Succeeded { commit, usage } => {
            if cancel_requested(&pool, run_id).await {
                let _ = sqlx::query(
                    "UPDATE agent_runs SET state = 'canceled', usage_tokens = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                )
                .bind(run_id)
                .bind(usage.tokens as i64)
                .execute(&pool)
                .await;
            } else if commit.is_some() {
                let _ = std::process::Command::new("git")
                    .current_dir(work_dir.path())
                    .args(["push", "origin", &format!("HEAD:refs/heads/{branch}")])
                    .status();
                let _ = sqlx::query(
                    "UPDATE agent_runs SET state = 'succeeded', result_commit_oid = $2, usage_tokens = $3, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                )
                .bind(run_id)
                .bind(commit)
                .bind(usage.tokens as i64)
                .execute(&pool)
                .await;
            } else {
                let _ = sqlx::query(
                    "UPDATE agent_runs SET state = 'succeeded', usage_tokens = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                )
                .bind(run_id)
                .bind(usage.tokens as i64)
                .execute(&pool)
                .await;
            }
            let _ = repo_id;
        }
        RunOutcome::Canceled { usage } => {
            let _ = sqlx::query(
                "UPDATE agent_runs SET state = 'canceled', usage_tokens = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(run_id)
            .bind(usage.tokens as i64)
            .execute(&pool)
            .await;
        }
        RunOutcome::Failed { code, .. } => {
            let _ = mark_failed(&pool, run_id, &code).await;
        }
    }
}

async fn normalize_cancel_outcome(pool: &PgPool, run_id: Uuid, outcome: RunOutcome) -> RunOutcome {
    let row = sqlx::query_as::<_, (String,)>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await;
    let Ok(Some((state,))) = row else {
        return outcome;
    };
    if state != "cancel_requested" {
        return outcome;
    }
    match outcome {
        RunOutcome::Canceled { .. } => outcome,
        RunOutcome::Failed { .. } | RunOutcome::Succeeded { .. } => {
            let usage = match &outcome {
                RunOutcome::Succeeded { usage, .. } => usage.clone(),
                RunOutcome::Failed { .. } => UsageTotals {
                    tokens: 0,
                    microusd: 0,
                    budget_unknown: true,
                },
                RunOutcome::Canceled { usage } => usage.clone(),
            };
            RunOutcome::Canceled { usage }
        }
    }
}

async fn cancel_requested(pool: &PgPool, run_id: Uuid) -> bool {
    matches!(
        sqlx::query_as::<_, (String,)>("SELECT state FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(pool)
            .await,
        Ok(Some((state,))) if state == "cancel_requested"
    )
}

async fn mark_failed(pool: &PgPool, run_id: Uuid, code: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE agent_runs SET state = 'failed', error_code = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(run_id)
    .bind(code)
    .execute(pool)
    .await?;
    Ok(())
}
