//! Protected environment awaiting_approval gate and deployment_approve state machine.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use rmcp::model::CallToolRequestParams;
use rmcp::{
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ClientLifecycleMode, ClientServiceExt,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tower::ServiceExt as TowerServiceExt;
use uuid::Uuid;

fn tool_data(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let v = result
        .structured_content
        .as_ref()
        .expect("structured")
        .clone();
    v.get("data").cloned().unwrap_or(v)
}

async fn mcp_client(
    app: &axum::Router,
    token: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientConfig> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut headers = HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
            .custom_headers(headers),
    );
    rmcp::model::ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                legacy_version: Some(rmcp::model::ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("client")
}

async fn pat(app: &axum::Router, cookie: &str, name: &str) -> String {
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(format!(r#"{{"name":"{name}","org":"demo"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(pat_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    pat_json["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn protected_deploy_approval_gate_audit_and_permissions() {
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
                .body(Body::from(r#"{"slug":"prot-deploy","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let (org_id, repo_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT o.id, r.id FROM organizations o
         JOIN repositories r ON r.org_id = o.id
         WHERE o.slug = 'demo' AND r.slug_normalized = 'prot-deploy'",
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
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let manifest_sha = "a".repeat(64);
    let pipe_id = Uuid::new_v4();
    let oid = "b".repeat(40);
    let artifact_bytes = b"prot-bundle";
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
    let member_pass = "Member-Deploy-Strong-22!";
    let hash = musehub_app::hash_password_for_tests(member_pass).unwrap();
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'deployer', 'deployer', 'Deployer', 'active')",
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
    let member_login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"handle": "deployer", "password": member_pass}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let member_cookie = member_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let member_token = pat(&app, &member_cookie, "member-dep").await;
    let admin_token = pat(&app, &admin_cookie, "admin-dep").await;
    let member_client = mcp_client(&app, &member_token).await;
    let start = member_client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_start").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.to_string())),
                    ("repository_id".into(), json!(repo_id.to_string())),
                    ("environment_id".into(), json!(prod_env.0.to_string())),
                    ("release_id".into(), json!(release_id.to_string())),
                    ("idempotency_key".into(), json!("prot-start-1")),
                ]),
            ),
        )
        .await
        .expect("start");
    assert!(!start.is_error.unwrap_or(true));
    let start_data = tool_data(&start);
    assert_eq!(start_data["state"], "awaiting_approval");
    assert_eq!(start_data["approval_required"], true);
    assert_eq!(start_data["plan_hash"], manifest_sha);
    let deployment_id = start_data["id"].as_str().unwrap();
    let dep_state: (String,) = sqlx::query_as("SELECT state FROM deployments WHERE id = $1")
        .bind(Uuid::parse_str(deployment_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dep_state.0, "awaiting_approval");
    let audit_wait: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM audit_events WHERE resource_type = 'deployment' AND action = 'deployment.awaiting_approval'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audit_wait.0 >= 1);
    let self_approve = member_client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_approve").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.to_string())),
                    ("repository_id".into(), json!(repo_id.to_string())),
                    ("deployment_id".into(), json!(deployment_id)),
                    ("expected_plan_hash".into(), json!(manifest_sha)),
                    ("decision".into(), json!("approve")),
                    ("reason".into(), json!("self")),
                    ("idempotency_key".into(), json!("self-approve")),
                ]),
            ),
        )
        .await
        .expect("self approve call");
    assert!(self_approve.is_error.unwrap_or(true));
    let fake_org = Uuid::new_v4();
    let admin_client = mcp_client(&app, &admin_token).await;
    let cross = admin_client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_approve").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(fake_org.to_string())),
                    ("repository_id".into(), json!(repo_id.to_string())),
                    ("deployment_id".into(), json!(deployment_id)),
                    ("expected_plan_hash".into(), json!(manifest_sha)),
                    ("decision".into(), json!("approve")),
                    ("reason".into(), json!("cross")),
                    ("idempotency_key".into(), json!("cross-approve")),
                ]),
            ),
        )
        .await
        .expect("cross");
    assert!(cross.is_error.unwrap_or(true));
    let approved = admin_client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_approve").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.to_string())),
                    ("repository_id".into(), json!(repo_id.to_string())),
                    ("deployment_id".into(), json!(deployment_id)),
                    ("expected_plan_hash".into(), json!(manifest_sha)),
                    ("decision".into(), json!("approve")),
                    ("reason".into(), json!("ok")),
                    ("idempotency_key".into(), json!("approve-1")),
                ]),
            ),
        )
        .await
        .expect("approve");
    assert!(!approved.is_error.unwrap_or(true));
    let after: (String,) = sqlx::query_as("SELECT state FROM deployments WHERE id = $1")
        .bind(Uuid::parse_str(deployment_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        after.0 == "succeeded" || after.0 == "failed",
        "must have executed after approval, got {}",
        after.0
    );
    assert_ne!(after.0, "awaiting_approval");
    let approvals: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM deployment_approvals WHERE deployment_id = $1 AND decision = 'approve'",
    )
    .bind(Uuid::parse_str(deployment_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(approvals.0, 1);
    let audit_approve: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'deployment.approve' AND resource_id = $1",
    )
    .bind(Uuid::parse_str(deployment_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_approve.0, 1);
    admin_client.cancel().await.ok();
    member_client.cancel().await.ok();
}
