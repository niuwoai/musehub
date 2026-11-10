//! Repo-scoped deployment list HTTP (session auth, pagination, filters).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

async fn get_json(
    app: &axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let val: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, val)
}

#[tokio::test]
async fn deployment_list_scoped_paginated_and_denied() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
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
                .body(Body::from(r#"{"slug":"dep-list","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let (org_id, repo_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT o.id, r.id FROM organizations o
         JOIN repositories r ON r.org_id = o.id
         WHERE o.slug = 'demo' AND r.slug_normalized = 'dep-list'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    musehub_app::seed_repo_environment(&pool, org_id, repo_id)
        .await
        .unwrap();
    let prod_env: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'production'")
            .bind(repo_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let test_env: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'test'")
            .bind(repo_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let release_id = Uuid::new_v4();
    let manifest = "e".repeat(64);
    let oid = "f".repeat(40);
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
    .bind(org_id)
    .bind(repo_id)
    .bind(&oid)
    .bind(admin_id.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'1.0.0','v1',$4,$5,'published',$6)",
    )
    .bind(release_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&oid)
    .bind(pipe_id)
    .bind(&manifest)
    .execute(&pool)
    .await
    .unwrap();
    for i in 0..3 {
        sqlx::query(
            "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, plan_hash)
             VALUES ($1,$2,$3,$4,$5,'compose-mock',$6,$7)",
        )
        .bind(Uuid::new_v4())
        .bind(org_id)
        .bind(repo_id)
        .bind(if i == 0 { prod_env.0 } else { test_env.0 })
        .bind(release_id)
        .bind(if i == 0 { "awaiting_approval" } else { "succeeded" })
        .bind(if i == 0 {
            Some(manifest.clone())
        } else {
            None::<String>
        })
        .execute(&pool)
        .await
        .unwrap();
    }
    let (ok, body) = get_json(
        &app,
        "/api/v1/orgs/demo/repos/dep-list/deployments?limit=2",
        Some(&admin_cookie),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    assert_eq!(body["has_more"], true);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    let (pending, pending_body) = get_json(
        &app,
        "/api/v1/orgs/demo/repos/dep-list/deployments?state=awaiting_approval",
        Some(&admin_cookie),
    )
    .await;
    assert_eq!(pending, StatusCode::OK);
    assert_eq!(pending_body["items"].as_array().unwrap().len(), 1);
    assert_eq!(pending_body["items"][0]["state"], "awaiting_approval");
    assert_eq!(pending_body["items"][0]["plan_hash"], manifest);

    let outsider_id = Uuid::new_v4();
    let outsider_pass = "Outsider-List-Mbr-12!";
    let hash = musehub_app::hash_password_for_tests(outsider_pass).unwrap();
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'list-outsider', 'list-outsider', 'Out', 'active')",
    )
    .bind(outsider_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_credentials (id, principal_id, password_hash, password_changed_at)
         VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
    )
    .bind(Uuid::new_v4())
    .bind(outsider_id)
    .bind(hash)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status)
         VALUES ($1, $2, $3, 'member', 'active')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(outsider_id)
    .execute(&pool)
    .await
    .unwrap();
    let outsider_cookie = common::login_cookie(&app, "list-outsider", outsider_pass).await;
    let (deny, _) = get_json(
        &app,
        "/api/v1/orgs/demo/repos/dep-list/deployments",
        Some(&outsider_cookie),
    )
    .await;
    assert_eq!(deny, StatusCode::FORBIDDEN);

    let wrong_slug: (StatusCode, _) = get_json(
        &app,
        "/api/v1/orgs/demo/repos/no-such/deployments",
        Some(&admin_cookie),
    )
    .await;
    assert_eq!(wrong_slug.0, StatusCode::NOT_FOUND);
}
