use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use musehub_agent_adapter::{AgentTask, RunOutcome, UsageTotals, REFERENCE_ADAPTER};

use crate::agent_adapter::spawn_agent_adapter;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::lease::finalize_agent_run_state;

async fn update_agent_progress(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    progress: &str,
) {
    if let Err(error) = sqlx::query(
        "UPDATE agent_runs SET recent_progress = $4, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3 AND state = 'running'",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(epoch)
    .bind(progress)
    .execute(pool)
    .await
    {
        tracing::warn!(run_id = %run_id, %error, "failed to update AgentRun progress");
    }
}

const DEFAULT_AGENT_GIT_NAME: &str = "MuseHub Agent";
const DEFAULT_AGENT_GIT_EMAIL: &str = "agent@musehub.local";

fn ensure_agent_git_identity(work_dir: &Path) -> Result<(), String> {
    for (key, default) in [
        ("user.name", DEFAULT_AGENT_GIT_NAME),
        ("user.email", DEFAULT_AGENT_GIT_EMAIL),
    ] {
        let configured = std::process::Command::new("git")
            .args(["config", "--get", key])
            .current_dir(work_dir)
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty());
        if configured {
            continue;
        }
        let status = std::process::Command::new("git")
            .args(["config", "--local", key, default])
            .current_dir(work_dir)
            .status()
            .map_err(|error| format!("could not configure {key}: {error}"))?;
        if !status.success() {
            return Err(format!("could not configure {key}"));
        }
    }
    Ok(())
}

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

async fn fenced_fail(pool: &PgPool, run_id: Uuid, instance_id: Uuid, epoch: i64, code: &str) {
    fenced_fail_with_usage(pool, run_id, instance_id, epoch, code, None, None).await;
}

async fn fenced_fail_with_usage(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    code: &str,
    usage_tokens: Option<i64>,
    usage_evidence: Option<&serde_json::Value>,
) {
    match finalize_agent_run_state(
        pool,
        run_id,
        instance_id,
        epoch,
        "failed",
        Some(code),
        None,
        usage_tokens,
        usage_evidence,
    )
    .await
    {
        Ok(false) => {
            let _ = finalize_agent_run_state(
                pool,
                run_id,
                instance_id,
                epoch,
                "canceled",
                None,
                None,
                usage_tokens,
                usage_evidence,
            )
            .await;
        }
        Err(error) => {
            tracing::error!(run_id = %run_id, %error, "failed to finalize AgentRun failure")
        }
        Ok(true) => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_agent_job(
    pool: PgPool,
    supervisor: Arc<AgentSupervisor>,
    instance_id: Uuid,
    run_id: Uuid,
    repo_id: Uuid,
    _storage_key: Uuid,
    git_root: PathBuf,
    reference_bin: PathBuf,
    branch: String,
    message: String,
    deadline: Duration,
    budget_tokens: Option<u64>,
    checkout_head_oid: Option<String>,
) {
    let checkout_head_oid = if checkout_head_oid.is_some() {
        checkout_head_oid
    } else {
        sqlx::query_as::<_, (Option<String>,)>("SELECT head_oid FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .and_then(|(oid,)| oid)
    };
    let work_lease = match crate::lease::try_begin_agent_work(&pool, run_id, instance_id).await {
        Ok(lease) => lease,
        Err(_) => {
            if let Some(lease) =
                crate::lease::lookup_agent_work_lease(&pool, run_id, instance_id).await
            {
                lease
            } else {
                let _ = sqlx::query(
                    "UPDATE agent_runs SET state = 'failed', error_code = 'lease_held_by_peer',
                 updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND state = 'queued'",
                )
                .bind(run_id)
                .execute(&pool)
                .await;
                return;
            }
        }
    };
    let epoch = work_lease.epoch;
    update_agent_progress(&pool, run_id, instance_id, epoch, "workspace_preparation").await;
    let _renew_guard =
        crate::lease::agent_lease_renew_guard(pool.clone(), run_id, instance_id, epoch);

    let storage_key =
        match sqlx::query_as::<_, (Uuid,)>("SELECT storage_key FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_optional(&pool)
            .await
        {
            Ok(Some((key,))) => key,
            _ => {
                fenced_fail(&pool, run_id, instance_id, epoch, "repo_metadata_failed").await;
                return;
            }
        };
    let work_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(_) => {
            fenced_fail(&pool, run_id, instance_id, epoch, "workspace_failed").await;
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
        fenced_fail(&pool, run_id, instance_id, epoch, "git_clone_failed").await;
        return;
    }
    if let Some(oid) = checkout_head_oid.as_deref() {
        let checkout_ok = std::process::Command::new("git")
            .current_dir(work_dir.path())
            .args(["checkout", "-B", &branch])
            .status()
            .is_ok_and(|s| s.success());
        let reset_ok = std::process::Command::new("git")
            .current_dir(work_dir.path())
            .args(["reset", "--hard", oid])
            .status()
            .is_ok_and(|s| s.success());
        if !checkout_ok || !reset_ok {
            fenced_fail(
                &pool,
                run_id,
                instance_id,
                epoch,
                "git_head_checkout_failed",
            )
            .await;
            return;
        }
    } else {
        let checkout_ok = std::process::Command::new("git")
            .current_dir(work_dir.path())
            .args(["checkout", "-B", &branch, "main"])
            .status()
            .is_ok_and(|s| s.success());
        if !checkout_ok {
            fenced_fail(
                &pool,
                run_id,
                instance_id,
                epoch,
                "git_branch_checkout_failed",
            )
            .await;
            return;
        }
    }

    if let Err(error) = ensure_agent_git_identity(work_dir.path()) {
        tracing::error!(run_id = %run_id, %error, "failed to configure AgentRun Git identity");
        fenced_fail(
            &pool,
            run_id,
            instance_id,
            epoch,
            "agent_git_identity_failed",
        )
        .await;
        return;
    }

    let message = if message.is_empty() {
        sqlx::query_as::<_, (String,)>("SELECT task_message FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .map(|(m,)| m)
            .unwrap_or_default()
    } else {
        message
    };
    update_agent_progress(&pool, run_id, instance_id, epoch, "adapter_running").await;
    let task = AgentTask {
        kind: "implementation".to_string(),
        message,
        branch: Some(branch.clone()),
    };
    let adapter_kind: (String,) =
        sqlx::query_as("SELECT adapter_kind FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap_or((REFERENCE_ADAPTER.to_string(),));
    let run = match spawn_agent_adapter(
        adapter_kind.0.as_str(),
        &task,
        work_dir.path(),
        &reference_bin,
    )
    .await
    {
        Ok(run) => run,
        Err(code) => {
            fenced_fail(&pool, run_id, instance_id, epoch, code).await;
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
    let deadline = std::env::var("MUSEHUB_AGENT_RUN_DEADLINE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(deadline);
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
            fenced_fail(&pool, run_id, instance_id, epoch, "adapter_failed").await;
            supervisor.inner.lock().await.remove(&run_id);
            poll.abort();
            return;
        }
    };
    poll.abort();
    supervisor.inner.lock().await.remove(&run_id);

    match outcome {
        RunOutcome::Succeeded {
            commit: Some(commit),
            usage,
        } => {
            publish_agent_commit(
                &pool,
                AgentCommitPublication {
                    run_id,
                    repo_id,
                    instance_id,
                    epoch,
                    work_dir: work_dir.path(),
                    branch: &branch,
                    commit: &commit,
                    usage,
                },
            )
            .await;
        }
        RunOutcome::Succeeded {
            commit: None,
            usage,
        } => {
            match finalize_agent_run_state(
                &pool,
                run_id,
                instance_id,
                epoch,
                "succeeded",
                None,
                None,
                Some(usage.tokens as i64),
                usage.evidence.as_ref(),
            )
            .await
            {
                Ok(false) => {
                    let _ = finalize_agent_run_state(
                        &pool,
                        run_id,
                        instance_id,
                        epoch,
                        "canceled",
                        None,
                        None,
                        Some(usage.tokens as i64),
                        usage.evidence.as_ref(),
                    )
                    .await;
                }
                Err(error) => {
                    tracing::error!(run_id = %run_id, %error, "failed to finalize AgentRun success")
                }
                Ok(true) => {}
            }
            let _ = repo_id;
        }
        RunOutcome::Canceled { usage } => {
            let _ = finalize_agent_run_state(
                &pool,
                run_id,
                instance_id,
                epoch,
                "canceled",
                None,
                None,
                Some(usage.tokens as i64),
                usage.evidence.as_ref(),
            )
            .await;
        }
        RunOutcome::Failed {
            code,
            detail,
            usage,
        } => {
            if matches!(
                code.as_str(),
                "agent_commit_missing"
                    | "agent_head_unreadable"
                    | "agent_changes_uncommitted"
                    | "agent_worktree_status_unavailable"
            ) {
                tracing::warn!(
                    run_id = %run_id,
                    detail = %detail,
                    "Codex completed without a publishable commit"
                );
            } else if code == "agent_adapter_failed" {
                tracing::error!(
                    run_id = %run_id,
                    detail = %detail,
                    "agent adapter process exited unsuccessfully"
                );
            }
            let usage_tokens = usage.as_ref().map(|usage| usage.tokens as i64);
            let usage_evidence = usage.as_ref().and_then(|usage| usage.evidence.as_ref());
            fenced_fail_with_usage(
                &pool,
                run_id,
                instance_id,
                epoch,
                &code,
                usage_tokens,
                usage_evidence,
            )
            .await;
        }
    }
}

struct AgentCommitPublication<'a> {
    run_id: Uuid,
    repo_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    work_dir: &'a Path,
    branch: &'a str,
    commit: &'a str,
    usage: UsageTotals,
}

async fn publish_agent_commit(pool: &PgPool, publication: AgentCommitPublication<'_>) {
    let AgentCommitPublication {
        run_id,
        repo_id,
        instance_id,
        epoch,
        work_dir,
        branch,
        commit,
        usage,
    } = publication;
    update_agent_progress(pool, run_id, instance_id, epoch, "commit_publishing").await;
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(run_id = %run_id, %error, "failed to begin AgentRun publish transaction");
            return;
        }
    };
    let state = match sqlx::query_as::<_, (String,)>(
        "SELECT state FROM agent_runs
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3
         FOR UPDATE",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(epoch)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(row) => row.map(|(state,)| state),
        Err(error) => {
            tracing::error!(run_id = %run_id, %error, "failed to lock AgentRun before publish");
            return;
        }
    };
    let Some(state) = state else {
        return;
    };
    if state == "cancel_requested" {
        let update = sqlx::query(
            "UPDATE agent_runs SET state = 'canceled', usage_tokens = $4,
             usage_evidence = COALESCE($5, usage_evidence), updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3
               AND state = 'cancel_requested'",
        )
        .bind(run_id)
        .bind(instance_id)
        .bind(epoch)
        .bind(usage.tokens as i64)
        .bind(usage.evidence.as_ref())
        .execute(&mut *tx)
        .await;
        match update {
            Ok(result) if result.rows_affected() == 1 => {
                if let Err(error) = tx.commit().await {
                    tracing::error!(run_id = %run_id, %error, "failed to commit canceled AgentRun");
                }
            }
            Ok(_) => {
                let _ = tx.rollback().await;
            }
            Err(error) => {
                tracing::error!(run_id = %run_id, %error, "failed to finalize canceled AgentRun");
            }
        }
        return;
    }
    if state != "running" {
        let _ = tx.rollback().await;
        return;
    }
    if let Err(error) =
        crate::repository_lock::lock_repository_in_transaction(&mut tx, repo_id).await
    {
        tracing::error!(run_id = %run_id, %error, "failed to acquire repository write lock for AgentRun publish");
        let _ = tx.rollback().await;
        return;
    }

    let work_dir = work_dir.to_path_buf();
    let target_ref = format!("HEAD:refs/heads/{branch}");
    let push = tokio::task::spawn_blocking(move || {
        std::process::Command::new("git")
            .current_dir(work_dir)
            .args(["push", "origin", &target_ref])
            .status()
    })
    .await;
    let push_ok = match push {
        Ok(Ok(status)) => status.success(),
        Ok(Err(error)) => {
            tracing::error!(run_id = %run_id, %error, "AgentRun git push failed to start");
            false
        }
        Err(error) => {
            tracing::error!(run_id = %run_id, %error, "AgentRun git push task failed");
            false
        }
    };
    let (terminal_state, error_code, result_commit) = if push_ok {
        ("succeeded", None, Some(commit))
    } else {
        ("failed", Some("git_push_failed"), None)
    };
    let update = sqlx::query(
        "UPDATE agent_runs SET state = $4, error_code = $5,
         result_commit_oid = COALESCE($6, result_commit_oid),
         usage_tokens = COALESCE($7, usage_tokens), usage_evidence = COALESCE($8, usage_evidence),
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3 AND state = 'running'",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(epoch)
    .bind(terminal_state)
    .bind(error_code)
    .bind(result_commit)
    .bind(usage.tokens as i64)
    .bind(usage.evidence.as_ref())
    .execute(&mut *tx)
    .await;
    match update {
        Ok(result) if result.rows_affected() == 1 => {
            if let Err(error) = tx.commit().await {
                tracing::error!(run_id = %run_id, %error, "failed to commit AgentRun publish result");
            }
        }
        Ok(_) => {
            let _ = tx.rollback().await;
        }
        Err(error) => {
            tracing::error!(run_id = %run_id, %error, "failed to finalize AgentRun publish result");
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
                RunOutcome::Failed { usage, .. } => usage.clone().unwrap_or(UsageTotals {
                    tokens: 0,
                    microusd: 0,
                    budget_unknown: true,
                    microusd_unknown: true,
                    evidence: None,
                }),
                RunOutcome::Canceled { usage } => usage.clone(),
            };
            RunOutcome::Canceled { usage }
        }
    }
}
