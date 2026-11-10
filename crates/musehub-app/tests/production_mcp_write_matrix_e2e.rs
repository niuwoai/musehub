//! MCP write tools: concurrent idempotency, deployment guards via product `/mcp`.

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
use tokio::net::TcpListener;
use tower::ServiceExt as TowerServiceExt;
use uuid::Uuid;

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

async fn issue_pat(app: &axum::Router, cookie: &str, name: &str) -> String {
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
async fn mcp_issue_create_concurrent_same_idempotency_key_once() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
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
                .body(Body::from(r#"{"slug":"mcp-write","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let token = issue_pat(&app, &cookie, "write-concurrent").await;
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'mcp-write'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client_a = mcp_client(&app, &token).await;
    let client_b = mcp_client(&app, &token).await;
    let key = "oci-concurrent-idem-v1";
    let args = serde_json::Map::from_iter([
        ("org_id".into(), json!(org_id.0.to_string())),
        ("repository_id".into(), json!(repo_id.0.to_string())),
        ("idempotency_key".into(), json!(key)),
        ("title".into(), json!("once")),
        ("body".into(), json!("")),
    ]);
    let (a, b) = tokio::join!(
        client_a.call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(args.clone())
        ),
        client_b.call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(args.clone())
        ),
    );
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM issues WHERE repo_id = $1")
        .bind(repo_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "concurrent idempotency must create one issue");
    let one_ok = [a.as_ref(), b.as_ref()].iter().any(|r| {
        r.is_ok()
            && r.as_ref()
                .map(|x| !x.is_error.unwrap_or(true))
                .unwrap_or(false)
    });
    if !one_ok {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let replay = client_a
            .call_tool(CallToolRequestParams::new("musehub_issue_create").with_arguments(args))
            .await
            .expect("replay");
        assert!(!replay.is_error.unwrap_or(true));
    }
    client_a.cancel().await.ok();
    client_b.cancel().await.ok();
}

#[tokio::test]
async fn mcp_deployment_start_rejects_draft_and_cross_tenant() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
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
                .body(Body::from(r#"{"slug":"mcp-dep","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let token = issue_pat(&app, &cookie, "dep-matrix").await;
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'mcp-dep'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let principal: (Uuid,) = sqlx::query_as("SELECT id FROM principals WHERE handle = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let env_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'test'")
            .bind(repo_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let pipe_id = Uuid::new_v4();
    let oid = "c".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'release','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(principal.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let release_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state)
         VALUES ($1,$2,$3,'0.0.1-mcp','v0-mcp',$4,$5,'draft')",
    )
    .bind(release_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(pipe_id)
    .execute(&pool)
    .await
    .unwrap();
    let client = mcp_client(&app, &token).await;
    let fake_org = Uuid::new_v4();
    let cross = client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_start").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(fake_org.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("environment_id".into(), json!(env_id.0.to_string())),
                    ("release_id".into(), json!(release_id.to_string())),
                    ("idempotency_key".into(), json!("dep-cross-tenant-v1")),
                ]),
            ),
        )
        .await
        .expect("cross");
    assert!(cross.is_error.unwrap_or(true));
    let draft = client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_start").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("environment_id".into(), json!(env_id.0.to_string())),
                    ("release_id".into(), json!(release_id.to_string())),
                    ("idempotency_key".into(), json!("dep-draft-v1")),
                ]),
            ),
        )
        .await
        .expect("draft");
    assert!(draft.is_error.unwrap_or(true));
    let dep_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM deployments WHERE release_id = $1")
            .bind(release_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dep_count.0, 0);
    client.cancel().await.ok();
}
