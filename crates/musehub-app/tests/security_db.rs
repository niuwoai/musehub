//! Database-backed authorization checks (requires Postgres).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use musehub_app::{prepare, product_router, AppState};
use musehub_migrate::{load_migrations, migrate_up};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

const BOOTSTRAP: &str = "Security-Db-Bootstrap-Strong-9!";

#[tokio::test]
async fn session_revoked_after_logout() {
    let database_url =
        std::env::var("MUSEHUB_TEST_DATABASE_URL").expect("MUSEHUB_TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .connect(&database_url)
        .await
        .expect("connect");
    let migrations =
        load_migrations(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations"))
            .expect("migrations");
    let _ = migrate_up(&pool, &migrations).await.expect("migrate");
    let git_root = tempfile::tempdir().expect("git");
    prepare(pool.clone(), git_root.path().to_path_buf(), Some(BOOTSTRAP))
        .await
        .expect("prepare");
    let state = Arc::new(AppState {
        pool: pool.clone(),
        git_root: git_root.path().to_path_buf(),
        bind_display: "127.0.0.1".to_string(),
        reference_adapter_bin: std::env::var("CARGO_BIN_EXE_musehub_agent_reference")
            .ok()
            .or_else(|| std::env::var("CARGO_BIN_EXE_musehub-agent-reference").ok())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-agent-reference")
            }),
    });
    let app = product_router(state);

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"handle":"admin","password":BOOTSTRAP}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("login");
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("logout");
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let me = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("me");
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
}
