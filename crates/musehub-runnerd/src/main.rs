//! Remote pipeline worker: claims queued runs via PAT and executes adapters locally.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use musehub_app::execute_pipeline_job;
use musehub_app::pipeline_supervisor::PipelineSupervisor;
use musehub_app::runner_api::{self, ClaimedPipelineRun};
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
    poll_secs: u64,
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
    let supervisor = Arc::new(PipelineSupervisor::new());
    tracing::info!(
        runner_id = %cfg.instance_id,
        api = %cfg.api_base,
        "musehub-runnerd starting"
    );
    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        match claim_run(&client, &cfg).await {
            Ok(Some(claimed)) => {
                tracing::info!(run_id = %claimed.run_id, "claimed pipeline run");
                if let Err(err) = run_claimed(&client, &cfg, &pool, &supervisor, claimed).await {
                    tracing::warn!(error = %err, "pipeline run failed");
                }
            }
            Ok(None) => {
                tokio::time::sleep(Duration::from_secs(cfg.poll_secs)).await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "claim failed");
                tokio::time::sleep(Duration::from_secs(cfg.poll_secs)).await;
            }
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
    let poll_secs = std::env::var("MUSEHUB_RUNNER_POLL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    Ok(Config {
        api_base: api_base.trim_end_matches('/').to_string(),
        pat,
        instance_id: runner_api::parse_runner_instance_from_env(),
        git_root,
        artifact_root,
        pipeline_bin,
        poll_secs,
    })
}

async fn claim_run(
    client: &reqwest::Client,
    cfg: &Config,
) -> Result<Option<ClaimedPipelineRun>, String> {
    let url = format!("{}/api/v1/runner/pipeline-runs/claim", cfg.api_base);
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
    if !resp.status().is_success() {
        return Err(format!("claim status {}", resp.status()));
    }
    let claimed: ClaimedPipelineRun = resp.json().await.map_err(|e| e.to_string())?;
    Ok(Some(claimed))
}

async fn run_claimed(
    client: &reqwest::Client,
    cfg: &Config,
    pool: &sqlx::PgPool,
    supervisor: &Arc<PipelineSupervisor>,
    claimed: ClaimedPipelineRun,
) -> Result<(), String> {
    let renew_client = client.clone();
    let renew_cfg = cfg.clone();
    let renew_run = claimed.run_id;
    let renew_epoch = claimed.lease_epoch;
    let renew_instance = cfg.instance_id;
    let renew_handle = tokio::spawn(async move {
        let interval = Duration::from_secs(runner_lease_renew_interval_secs());
        loop {
            tokio::time::sleep(interval).await;
            let url = format!(
                "{}/api/v1/runner/pipeline-runs/{}/lease/renew",
                renew_cfg.api_base, renew_run
            );
            let body = serde_json::json!({ "lease_epoch": renew_epoch });
            let auth = match bearer(&renew_cfg.pat) {
                Ok(h) => h,
                Err(_) => break,
            };
            let resp = renew_client
                .post(url)
                .header(AUTHORIZATION, auth)
                .header("x-musehub-runner-id", renew_instance.to_string())
                .json(&body)
                .send()
                .await;
            if resp.is_err() || !resp.unwrap().status().is_success() {
                break;
            }
        }
    });
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
    renew_handle.abort();
    Ok(())
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
