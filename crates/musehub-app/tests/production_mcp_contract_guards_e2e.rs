//! Negative paths for new contract tools (cross-tenant / auth).

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use http_body_util::BodyExt;
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

async fn mcp_client(app: &axum::Router, token: &str) -> rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientConfig> {
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
async fn contract_repository_tree_rejects_foreign_repo() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    for slug in ["tree-a", "tree-b"] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(format!(r#"{{"slug":"{slug}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_a: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'tree-a'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_b: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'tree-b'")
        .fetch_one(&pool)
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
                .body(Body::from(
                    json!({
                        "name": "sel-tree",
                        "org": "demo",
                        "scopes": ["repo:read"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_a.0.to_string()]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let token: String = serde_json::from_slice::<serde_json::Value>(
        &pat_resp.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let main_oid = {
        let sk: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
            .bind(repo_b.0)
            .fetch_one(&pool)
            .await
            .unwrap();
        let bare = musehub_app::bare_repo_path(git_root.path(), &sk.0);
        let out = std::process::Command::new("git")
            .args([
                "--git-dir",
                bare.to_str().unwrap(),
                "rev-parse",
                "refs/heads/main",
            ])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let client = mcp_client(&app, &token).await;
    let denied = client
        .call_tool(CallToolRequestParams::new("musehub_repository_tree").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_b.0.to_string())),
                ("commit_oid".into(), json!(main_oid)),
            ]),
        ))
        .await
        .expect("call");
    assert!(
        denied.is_error.unwrap_or(false) || denied.structured_content.is_none(),
        "selected PAT must not read other repo tree"
    );
    client.cancel().await.ok();
}
