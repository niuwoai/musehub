//! GET /releases lists published rows for repo members.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn list_releases_returns_seeded_rows() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &boot).await;
    let admin = common::login_cookie(&app, "admin", &boot).await;
    let repo = format!("rel-list-{}", Uuid::new_v4().simple());
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "slug": repo }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE org_id = $1 AND slug = $2")
            .bind(org_id.0)
            .bind(&repo)
            .fetch_one(&pool)
            .await
            .unwrap();
    let oid = "c".repeat(40);
    let pipe_id = Uuid::new_v4();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'release','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(admin_id.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let release_id = Uuid::new_v4();
    let manifest = "d".repeat(64);
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'1.0.0','v1.2.3',$4,$5,'published',$6)",
    )
    .bind(release_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(pipe_id)
    .bind(&manifest)
    .execute(&pool)
    .await
    .unwrap();

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/releases"))
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list.into_body(), usize::MAX)
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["tag_name"], "v1.2.3");
    assert_eq!(rows[0]["state"], "published");
}
