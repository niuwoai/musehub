//! Protected deployment approval over session HTTP (console path).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

async fn post_json(
    app: &axum::Router,
    uri: &str,
    cookie: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
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
async fn protected_deploy_approval_http_permissions_and_refresh() {
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
                .body(Body::from(r#"{"slug":"prot-http","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let (org_id, repo_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT o.id, r.id FROM organizations o
         JOIN repositories r ON r.org_id = o.id
         WHERE o.slug = 'demo' AND r.slug_normalized = 'prot-http'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    musehub_app::seed_repo_environment(&pool, org_id, repo_id)
        .await
        .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let manifest_sha = "c".repeat(64);
    let pipe_id = Uuid::new_v4();
    let oid = "d".repeat(40);
    let artifact_bytes = b"prot-http-bundle";
    let sha256 = Sha256::digest(artifact_bytes);
    let sha_hex: String = sha256.iter().map(|b| format!("{:02x}", b)).collect();
    let artifact_dir = git_root
        .path()
        .join("artifacts")
        .join(org_id.to_string())
        .join(repo_id.to_string());
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("bundle.tar"), artifact_bytes).unwrap();
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
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, sha256, byte_length, storage_path, state)
         VALUES ($1,$2,$3,$4,'bundle.tar',$5,$6,$7,'ready')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(pipe_id)
    .bind(&sha_hex)
    .bind(artifact_bytes.len() as i64)
    .bind(artifact_dir.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .unwrap();
    let release_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'1.0.0','v1',$4,$5,'published',$6)",
    )
    .bind(release_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&oid)
    .bind(pipe_id)
    .bind(&manifest_sha)
    .execute(&pool)
    .await
    .unwrap();

    let member_id = Uuid::new_v4();
    let cred_id = Uuid::new_v4();
    let member_pass = "Member-Http-Deploy-22!";
    let hash = musehub_app::hash_password_for_tests(member_pass).unwrap();
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'http-deployer', 'http-deployer', 'Http Deployer', 'active')",
    )
    .bind(member_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_credentials (id, principal_id, password_hash, password_changed_at)
         VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
    )
    .bind(cred_id)
    .bind(member_id)
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
    .bind(member_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1, $2, $3, $4, 'developer')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(member_id)
    .execute(&pool)
    .await
    .unwrap();
    let outsider_id = Uuid::new_v4();
    let outsider_pass = "Outsider-Http-12!";
    let outsider_hash = musehub_app::hash_password_for_tests(outsider_pass).unwrap();
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'http-outsider', 'http-outsider', 'Outsider', 'active')",
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
    .bind(outsider_hash)
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

    let member_cookie = common::login_cookie(&app, "http-deployer", member_pass).await;
    let outsider_cookie = common::login_cookie(&app, "http-outsider", outsider_pass).await;
    let attempt_id = Uuid::new_v4();
    let (start_status, start_body) = post_json(
        &app,
        "/api/v1/orgs/demo/repos/prot-http/deployments",
        Some(&member_cookie),
        json!({
            "release_id": release_id.to_string(),
            "environment_slug": "production",
            "attempt_id": attempt_id.to_string(),
        }),
    )
    .await;
    assert_eq!(start_status, StatusCode::ACCEPTED);
    assert_eq!(start_body["state"], "awaiting_approval");
    assert_eq!(start_body["approval_required"], true);
    assert_eq!(start_body["plan_hash"], manifest_sha);
    let deployment_id = start_body["id"].as_str().unwrap();

    let (deny_status, _) = post_json(
        &app,
        &format!("/api/v1/orgs/demo/repos/prot-http/deployments/{deployment_id}/approval"),
        Some(&outsider_cookie),
        json!({
            "expected_plan_hash": manifest_sha,
            "decision": "approve",
            "reason": "no grant",
        }),
    )
    .await;
    assert_eq!(deny_status, StatusCode::FORBIDDEN);

    let (self_status, _) = post_json(
        &app,
        &format!("/api/v1/orgs/demo/repos/prot-http/deployments/{deployment_id}/approval"),
        Some(&member_cookie),
        json!({
            "expected_plan_hash": manifest_sha,
            "decision": "approve",
            "reason": "self",
        }),
    )
    .await;
    assert_eq!(self_status, StatusCode::CONFLICT);

    let (bad_hash_status, _) = post_json(
        &app,
        &format!("/api/v1/orgs/demo/repos/prot-http/deployments/{deployment_id}/approval"),
        Some(&admin_cookie),
        json!({
            "expected_plan_hash": "0".repeat(64),
            "decision": "approve",
            "reason": "bad",
        }),
    )
    .await;
    assert_eq!(bad_hash_status, StatusCode::BAD_REQUEST);

    let (ok_status, ok_body) = post_json(
        &app,
        &format!("/api/v1/orgs/demo/repos/prot-http/deployments/{deployment_id}/approval"),
        Some(&admin_cookie),
        json!({
            "expected_plan_hash": manifest_sha,
            "decision": "approve",
            "reason": "ok",
        }),
    )
    .await;
    assert_eq!(ok_status, StatusCode::OK);
    assert_eq!(ok_body["decision"], "approve");
    assert_ne!(ok_body["state"], "awaiting_approval");

    let get = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/prot-http/deployments/{deployment_id}"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    let get_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(get.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(get_body["state"], ok_body["state"]);
    assert_eq!(get_body["approval_required"], false);

    let (again_status, _) = post_json(
        &app,
        &format!("/api/v1/orgs/demo/repos/prot-http/deployments/{deployment_id}/approval"),
        Some(&admin_cookie),
        json!({
            "expected_plan_hash": manifest_sha,
            "decision": "approve",
            "reason": "repeat",
        }),
    )
    .await;
    assert_eq!(again_status, StatusCode::CONFLICT);
}
