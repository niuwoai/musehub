//! MCP security: scope, cross-tenant, idempotency conflict, runner PAT blocked at HTTP gate.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http::HeaderValue;
use musehub_app::mcp::PRODUCTION_SCOPE;
use rmcp::{
    model::CallToolRequestParams,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ClientLifecycleMode, ClientServiceExt,
};
use serde_json::{json, Value};
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

#[tokio::test]
async fn runner_pat_gets_403_on_mcp_http() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let _cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let runner_token = musehub_app::pat::generate_pat();
    sqlx::query(
        "INSERT INTO access_tokens (id, principal_id, name, token_prefix, token_hash, scopes, expires_at, org_id)
         SELECT $1, p.id, 'runner-mcp', substring($2 from 1 for 12), $3,
                ARRAY['checks:write','repo:read']::text[], CURRENT_TIMESTAMP + interval '1 day', o.id
         FROM principals p, organizations o
         WHERE p.handle_normalized = 'admin' AND o.slug_normalized = 'demo'",
    )
    .bind(Uuid::new_v4())
    .bind(&runner_token)
    .bind(musehub_app::pat::hash_pat(&runner_token))
    .execute(&pool)
    .await
    .unwrap();
    let token = runner_token;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body("")
        .send()
        .await
        .expect("post mcp");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn idempotency_conflict_and_cross_org_denied() {
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
                .body(Body::from(r#"{"slug":"mcp-sec","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"mcp-write","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: Value = serde_json::from_slice(
        &axum::body::to_bytes(pat_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let token = pat_json["token"].as_str().unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'mcp-sec'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client = mcp_client(&app, token).await;
    let key = "idem-key-12345678";
    let args1 = json!({
        "org_id": org_id.0.to_string(),
        "repository_id": repo_id.0.to_string(),
        "idempotency_key": key,
        "title": "first",
        "body": ""
    });
    let r1 = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create")
                .with_arguments(args1.as_object().cloned().unwrap()),
        )
        .await
        .expect("create");
    assert!(!r1.is_error.unwrap_or(false));
    let args2 = json!({
        "org_id": org_id.0.to_string(),
        "repository_id": repo_id.0.to_string(),
        "idempotency_key": key,
        "title": "second",
        "body": ""
    });
    let conflict = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create")
                .with_arguments(args2.as_object().cloned().unwrap()),
        )
        .await;
    assert!(
        conflict.is_err()
            || conflict
                .as_ref()
                .ok()
                .and_then(|r| r.structured_content.as_ref())
                .and_then(|v| v.get("error"))
                .is_some(),
        "must not accept same key with different body"
    );
    let fake_org = Uuid::new_v4();
    let cross = client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_list").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), json!(fake_org.to_string()));
                m
            }),
        )
        .await
        .expect("cross list");
    let err = cross
        .structured_content
        .and_then(|v| v.get("error").cloned());
    assert!(
        err.as_ref()
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            == Some("PERMISSION_DENIED")
            || err
                .as_ref()
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
                == Some("NOT_FOUND"),
        "cross-tenant must not leak: {err:?}"
    );
    let caps = client
        .call_tool(CallToolRequestParams::new("musehub_capabilities_get"))
        .await
        .unwrap();
    let scope = caps
        .structured_content
        .and_then(|v| v.get("data").cloned())
        .and_then(|d| d.get("mcp_scope").and_then(Value::as_str).map(String::from));
    assert_eq!(scope.as_deref(), Some(PRODUCTION_SCOPE));
    client.cancel().await.ok();
}
