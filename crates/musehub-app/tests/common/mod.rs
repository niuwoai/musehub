#![allow(dead_code)]

pub mod git_ssh_fixture;
pub mod git_ssh_sshd;
pub mod git_ssh_util;
pub mod s1_git_push;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use musehub_app::agent_supervisor::new_supervisor;
use musehub_app::pipeline_supervisor::PipelineSupervisor;
use musehub_app::{boot_reconcile, lease, prepare, product_router, AppState};
use musehub_migrate::{load_migrations, migrate_down, migrate_up};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tower::ServiceExt;

pub async fn migrated_pool() -> PgPool {
    let database_url =
        std::env::var("MUSEHUB_TEST_DATABASE_URL").expect("MUSEHUB_TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("connect");
    let migrations =
        load_migrations(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations"))
            .expect("migrations");
    std::env::set_var("MUSEHUB_MIGRATE_TERMINATE_IDLE_PEERS", "1");
    let _ = migrate_down(&pool, &migrations).await;
    let _ = migrate_up(&pool, &migrations).await.expect("migrate");
    pool
}

pub fn bootstrap_password() -> String {
    std::env::var("MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD")
        .unwrap_or_else(|_| "Acceptance-Bootstrap-Strong-1!".to_string())
}

pub fn reference_adapter_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_musehub-agent-reference")
        .ok()
        .or_else(|| std::env::var("CARGO_BIN_EXE_musehub-agent-reference").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-agent-reference")
        })
}

pub fn compose_mock_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_musehub-compose-mock")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-compose-mock")
        })
}

pub fn compose_runner_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_musehub_compose_runner")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-compose-runner"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-compose-runner")
        })
}

pub async fn product_app(
    pool: PgPool,
    git_root: &std::path::Path,
    bootstrap_password: &str,
) -> Router {
    prepare(
        pool.clone(),
        git_root.to_path_buf(),
        Some(bootstrap_password),
    )
    .await
    .expect("prepare");
    boot_reconcile(&pool).await.expect("boot reconcile");
    let state = Arc::new(AppState {
        pool,
        git_root: git_root.to_path_buf(),
        artifact_root: git_root.join("artifacts"),
        bind_display: "127.0.0.1".to_string(),
        reference_adapter_bin: std::env::var("CARGO_BIN_EXE_musehub_agent_reference")
            .map(PathBuf::from)
            .ok()
            .filter(|p| p.is_file())
            .unwrap_or_else(reference_adapter_bin),
        pipeline_adapter_bin: std::env::var("MUSEHUB_PIPELINE_ADAPTER_BIN")
            .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-pipeline-oci"))
            .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-pipeline-reference"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-pipeline-reference")
            }),
        compose_adapter_bin: std::env::var("CARGO_BIN_EXE_musehub-compose-mock")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-compose-mock")
            }),
        compose_runner_bin: std::env::var("CARGO_BIN_EXE_musehub_compose_runner")
            .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-compose-runner"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-compose-runner")
            }),
        instance_id: lease::parse_instance_id(),
        supervisor: new_supervisor(),
        pipeline_supervisor: PipelineSupervisor::new(),
        require_pipeline_oci: musehub_app::require_pipeline_oci_from_env(),
        mcp_pat_slot: Arc::new(tokio::sync::RwLock::new(None)),
    });
    product_router(state)
}

pub async fn login_cookie(app: &Router, handle: &str, password: &str) -> String {
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"handle": handle, "password": password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("login");
    assert_eq!(login.status(), StatusCode::OK);
    login
        .headers()
        .get(header::SET_COOKIE)
        .expect("cookie")
        .to_str()
        .unwrap()
        .to_string()
}

pub fn branch_commit_count(bare: &std::path::Path, branch: &str) -> u64 {
    let verify = std::process::Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "show-ref",
            "--verify",
            &format!("refs/heads/{branch}"),
        ])
        .status()
        .expect("show-ref");
    if !verify.success() {
        return 0;
    }
    let output = std::process::Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "rev-list",
            "--count",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .expect("rev-list");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}
