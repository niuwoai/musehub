//! Remote worker: claims pipeline and agent runs via PAT; executes adapters locally (shared DB/git).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use musehub_app::agent_supervisor::AgentSupervisor;
use musehub_app::execute_agent_job;
use musehub_app::execute_pipeline_job;
use musehub_app::pipeline_supervisor::PipelineSupervisor;
use musehub_app::runner_api::{ClaimedAgentRun, ClaimedPipelineRun};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone)]
struct Config {
    api_base: String,
    pat: String,
    instance_id: Uuid,
    git_root: PathBuf,
    artifact_root: PathBuf,
    pipeline_bin: PathBuf,
    reference_adapter_bin: PathBuf,
    poll_secs: u64,
    work_kinds: WorkKinds,
}

#[derive(Clone, Copy, Debug)]
enum WorkKinds {
    PipelineOnly,
    AgentOnly,
    Both,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cfg = match load_config() {
        Ok(cfg) => cfg,
        Err(msg) => {
            tracing::error!(error = %msg, "invalid configuration");
            std::process::exit(78);
        }
    };
    if cfg.pat.is_empty() {
        tracing::error!("MUSEHUB_RUNNER_PAT is required");
        std::process::exit(78);
    }
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });
    let client = reqwest::Client::new();
    let pool = connect_pool().await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "database connect failed");
        std::process::exit(78);
    });
    let pipeline_supervisor = Arc::new(PipelineSupervisor::new());
    let agent_supervisor = Arc::new(AgentSupervisor::default());
    tracing::info!(
        runner_id = %cfg.instance_id,
        api = %cfg.api_base,
        work_kinds = ?cfg.work_kinds,
        "musehub-runnerd starting"
    );
    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        let mut claimed_work = false;
        if matches!(cfg.work_kinds, WorkKinds::Both | WorkKinds::PipelineOnly) {
            match claim_pipeline(&client, &cfg).await {
                Ok(Some(claimed)) => {
                    claimed_work = true;
                    tracing::info!(run_id = %claimed.run_id, "claimed pipeline run");
                    if let Err(err) =
                        run_pipeline(&client, &cfg, &pool, &pipeline_supervisor, claimed).await
                    {
                        tracing::warn!(error = %err, "pipeline run failed");
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(error = %err, "pipeline claim failed"),
            }
        }
        if !claimed_work && matches!(cfg.work_kinds, WorkKinds::Both | WorkKinds::AgentOnly) {
            match claim_agent(&client, &cfg).await {
                Ok(Some(claimed)) => {
                    claimed_work = true;
                    tracing::info!(run_id = %claimed.run_id, "claimed agent run");
                    if let Err(err) =
                        run_agent(&client, &cfg, &pool, &agent_supervisor, claimed).await
                    {
                        tracing::warn!(error = %err, "agent run failed");
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(error = %err, "agent claim failed"),
            }
        }
        if !claimed_work {
            tokio::time::sleep(Duration::from_secs(cfg.poll_secs)).await;
        }
    }
}

fn load_config() -> Result<Config, String> {
    let api_base =
        std::env::var("MUSEHUB_API_BASE").map_err(|_| "MUSEHUB_API_BASE required".to_string())?;
    let pat = std::env::var("MUSEHUB_RUNNER_PAT").unwrap_or_default();
    let git_root = std::env::var("MUSEHUB_GIT_ROOT")
        .map(PathBuf::from)
        .map_err(|_| "MUSEHUB_GIT_ROOT required".to_string())?;
    let artifact_root = std::env::var("MUSEHUB_ARTIFACT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| git_root.join("artifacts"));
    let pipeline_bin = std::env::var("MUSEHUB_PIPELINE_ADAPTER_BIN")
        .map(PathBuf::from)
        .map_err(|_| "MUSEHUB_PIPELINE_ADAPTER_BIN required".to_string())?;
    let reference_adapter_bin = std::env::var("MUSEHUB_REFERENCE_ADAPTER_BIN")
        .map(PathBuf::from)
        .map_err(|_| "MUSEHUB_REFERENCE_ADAPTER_BIN required for agent work".to_string())?;
    let poll_secs = std::env::var("MUSEHUB_RUNNER_POLL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let work_kinds = match std::env::var("MUSEHUB_RUNNER_WORK_KINDS")
        .unwrap_or_else(|_| "both".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "pipeline" => WorkKinds::PipelineOnly,
        "agent" => WorkKinds::AgentOnly,
        _ => WorkKinds::Both,
    };
    Ok(Config {
        api_base: api_base.trim_end_matches('/').to_string(),
        pat,
        instance_id: musehub_app::runner_api::parse_runner_instance_from_env(),
        git_root,
        artifact_root,
        pipeline_bin,
        reference_adapter_bin,
        poll_secs,
        work_kinds,
    })
}

async fn claim_pipeline(
    client: &reqwest::Client,
    cfg: &Config,
) -> Result<Option<ClaimedPipelineRun>, String> {
    let url = format!("{}/api/v1/runner/pipeline-runs/claim", cfg.api_base);
    claim_json(client, cfg, &url).await
}

async fn claim_agent(
    client: &reqwest::Client,
    cfg: &Config,
) -> Result<Option<ClaimedAgentRun>, String> {
    let url = format!("{}/api/v1/runner/agent-runs/claim", cfg.api_base);
    claim_json(client, cfg, &url).await
}

async fn claim_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    cfg: &Config,
    url: &str,
) -> Result<Option<T>, String> {
    let resp = client
        .post(url)
        .header(AUTHORIZATION, bearer(&cfg.pat)?)
        .header("x-musehub-runner-id", cfg.instance_id.to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if resp.status() == reqwest::StatusCode::CONFLICT {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("claim status {}", resp.status()));
    }
    let claimed: T = resp.json().await.map_err(|e| e.to_string())?;
    Ok(Some(claimed))
}

async fn run_pipeline(
    client: &reqwest::Client,
    cfg: &Config,
    pool: &sqlx::PgPool,
    supervisor: &Arc<PipelineSupervisor>,
    claimed: ClaimedPipelineRun,
) -> Result<(), String> {
    let renew = spawn_renew(
        client.clone(),
        cfg.clone(),
        "pipeline",
        claimed.run_id,
        claimed.lease_epoch,
    );
    execute_pipeline_job(
        pool.clone(),
        cfg.git_root.clone(),
        cfg.artifact_root.clone(),
        cfg.pipeline_bin.clone(),
        supervisor.clone(),
        cfg.instance_id,
        claimed.storage_key,
        claimed.run_id,
        claimed.job_id,
        claimed.org_id,
        claimed.repo_id,
        claimed.source_oid,
        claimed.run_kind,
    )
    .await;
    renew.abort();
    Ok(())
}

async fn run_agent(
    client: &reqwest::Client,
    cfg: &Config,
    pool: &sqlx::PgPool,
    supervisor: &Arc<AgentSupervisor>,
    claimed: ClaimedAgentRun,
) -> Result<(), String> {
    let renew = spawn_renew(
        client.clone(),
        cfg.clone(),
        "agent",
        claimed.run_id,
        claimed.lease_epoch,
    );
    let budget = claimed.budget_tokens_limit.map(|v| v as u64);
    execute_agent_job(
        pool.clone(),
        supervisor.clone(),
        cfg.instance_id,
        claimed.run_id,
        claimed.repo_id,
        claimed.storage_key,
        cfg.git_root.clone(),
        cfg.reference_adapter_bin.clone(),
        claimed.work_branch,
        claimed.task_message,
        Duration::from_secs(120),
        budget,
    )
    .await;
    renew.abort();
    Ok(())
}

fn spawn_renew(
    client: reqwest::Client,
    cfg: Config,
    kind: &'static str,
    run_id: Uuid,
    lease_epoch: i64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let interval = Duration::from_secs(runner_lease_renew_interval_secs());
        let path = match kind {
            "agent" => format!("/api/v1/runner/agent-runs/{run_id}/lease/renew"),
            _ => format!("/api/v1/runner/pipeline-runs/{run_id}/lease/renew"),
        };
        loop {
            tokio::time::sleep(interval).await;
            let url = format!("{}{}", cfg.api_base, path);
            let body = serde_json::json!({ "lease_epoch": lease_epoch });
            let auth = match bearer(&cfg.pat) {
                Ok(h) => h,
                Err(_) => break,
            };
            let resp = client
                .post(url)
                .header(AUTHORIZATION, auth)
                .header("x-musehub-runner-id", cfg.instance_id.to_string())
                .json(&body)
                .send()
                .await;
            if resp.is_err() || !resp.unwrap().status().is_success() {
                break;
            }
        }
    })
}

fn runner_lease_renew_interval_secs() -> u64 {
    if let Ok(raw) = std::env::var("MUSEHUB_RUNNER_LEASE_RENEW_SECS") {
        if let Ok(v) = raw.parse::<u64>() {
            if (1..=60).contains(&v) {
                return v;
            }
        }
    }
    let lease = std::env::var("MUSEHUB_LEASE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&s| (5..=300).contains(&s))
        .unwrap_or(45);
    std::cmp::min(15, (lease / 3).max(2) as u64)
}

fn bearer(pat: &str) -> Result<HeaderValue, String> {
    HeaderValue::from_str(&format!("Bearer {pat}")).map_err(|e| e.to_string())
}

async fn connect_pool() -> Result<sqlx::PgPool, String> {
    let url = std::env::var("MUSEHUB_DATABASE_URL")
        .map_err(|_| "MUSEHUB_DATABASE_URL required on runner for artifact metadata".to_string())?;
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .map_err(|e| e.to_string())
}
