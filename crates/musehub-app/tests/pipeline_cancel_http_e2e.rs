//! Session HTTP: cancel pipeline run (auth, path binding, terminal-state no-op).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn session_cancel_running_to_cancel_requested_and_late_cancel_no_op() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    r#"{"slug":"pipe-cancel-http","description":""}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let org_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let repo_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM repositories WHERE slug_normalized = 'pipe-cancel-http'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let admin_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let run_running = Uuid::new_v4();
    let run_failed = Uuid::new_v4();
    let oid = "e".repeat(40);
    for (rid, st) in [(run_running, "running"), (run_failed, "failed")] {
        sqlx::query(
            "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
             requested_by, run_attempt_id, run_kind)
             VALUES ($1,$2,$3,$4,'product-ci',$5,$6,$7,'release_build')",
        )
        .bind(rid)
        .bind(org_id)
        .bind(repo_id)
        .bind(&oid)
        .bind(st)
        .bind(admin_id)
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
    }

    let cancel_running = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-cancel-http/pipeline-runs/{run_running}/cancel"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancel_running.status(), StatusCode::OK);
    let run_st: (String, bool) =
        sqlx::query_as("SELECT state, cancel_requested FROM pipeline_runs WHERE id = $1")
            .bind(run_running)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(run_st.0, "cancel_requested");
    assert!(run_st.1);

    let late = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-cancel-http/pipeline-runs/{run_failed}/cancel"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(late.status(), StatusCode::OK);
    let failed_st: (String,) = sqlx::query_as("SELECT state FROM pipeline_runs WHERE id = $1")
        .bind(run_failed)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(failed_st.0, "failed");
}

#[tokio::test]
async fn session_cancel_requires_auth_and_repo_binding() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    for slug in ["pipe-cancel-a", "pipe-cancel-b"] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(format!(
                        r#"{{"slug":"{slug}","description":""}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    let repo_a: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM repositories WHERE slug_normalized = 'pipe-cancel-a'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let org_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let admin_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let run_id = Uuid::new_v4();
    let oid = "f".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','running',$5,$6,'release_build')",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_a)
    .bind(&oid)
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();

    let no_cookie = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-cancel-a/pipeline-runs/{run_id}/cancel"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_cookie.status(), StatusCode::UNAUTHORIZED);

    let wrong_repo = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-cancel-b/pipeline-runs/{run_id}/cancel"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_repo.status(), StatusCode::NOT_FOUND);
}
