#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use musehub_app::agent_supervisor::new_supervisor;
use musehub_app::{prepare, product_router, AppState};
use musehub_migrate::{load_migrations, migrate_up};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

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
    let _ = migrate_up(&pool, &migrations).await.expect("migrate");
    pool
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

pub fn bootstrap_password() -> String {
    std::env::var("MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD")
        .unwrap_or_else(|_| "Acceptance-Bootstrap-Strong-1!".to_string())
}

pub async fn product_app(
    pool: PgPool,
    git_root: &std::path::Path,
    bootstrap_password: &str,
) -> axum::Router {
    prepare(
        pool.clone(),
        git_root.to_path_buf(),
        Some(bootstrap_password),
    )
    .await
    .expect("prepare");
    let state = Arc::new(AppState {
        pool,
        git_root: git_root.to_path_buf(),
        bind_display: "127.0.0.1".to_string(),
        reference_adapter_bin: reference_adapter_bin(),
        supervisor: new_supervisor(),
    });
    product_router(state)
}
