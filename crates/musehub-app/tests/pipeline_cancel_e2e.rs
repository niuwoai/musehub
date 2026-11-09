//! Pipeline cancel transitions and release block.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn cancel_release_pipeline_blocks_release() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"pipe-cancel","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let merged_oid = "b".repeat(40);
    let org_id: uuid::Uuid = sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT id FROM organizations WHERE slug_normalized = 'demo'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let repo_id: uuid::Uuid = sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT id FROM repositories WHERE slug_normalized = 'pipe-cancel'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let admin_id: uuid::Uuid = sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT id FROM principals WHERE handle_normalized = 'admin'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let run_id = uuid::Uuid::new_v4();
    let attempt = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind, cancel_requested)
         VALUES ($1,$2,$3,$4,'product-ci','canceled',$5,$6,'release_build',TRUE)",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&merged_oid)
    .bind(admin_id)
    .bind(attempt)
    .execute(&pool)
    .await
    .unwrap();

    let rel = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pipe-cancel/releases")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "version": "0.0.1",
                        "tag_name": "v0.0.1",
                        "commit_oid": merged_oid,
                        "pipeline_run_id": run_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rel.status(), StatusCode::BAD_REQUEST);
}
