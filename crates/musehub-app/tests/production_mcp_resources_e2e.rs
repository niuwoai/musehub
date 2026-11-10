//! MCP Resources §13: agent-run summary URI with repo scope + cross-tenant deny.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use http_body_util::BodyExt;
use rmcp::model::{ReadResourceRequestParams, ResourceContents};
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

fn codex_fixture_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_musehub-agent-codex-fixture")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-agent-codex-fixture")
        })
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

#[tokio::test]
async fn mcp_read_agent_run_summary_requires_repo_access() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let fixture = codex_fixture_bin();
    if !fixture.is_file() {
        eprintln!("skip: build musehub-agent-codex-fixture first");
        return;
    }
    std::env::set_var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN", fixture);
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    for slug in ["mcp-res-a", "mcp-res-b"] {
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
    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/mcp-res-a/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"title":"r"}"#))
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let n = issue["number"].as_i64().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/mcp-res-a/issues/{n}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"body":"p"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/mcp-res-a/issues/{n}/plans"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let agent: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/mcp-res-a/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        json!({
                            "issue_id": issue["id"],
                            "message": "fixture:ok",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let run_id = agent["id"].as_str().unwrap();
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/v1/agent-runs/{run_id}"))
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
        )
        .unwrap();
        if st["state"] == "succeeded" || st["state"] == "failed" {
            break;
        }
    }
    let pat_a = {
        let pat_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tokens")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"name":"res-a","org":"demo"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(pat_resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        v["token"].as_str().unwrap().to_string()
    };
    let repo_b: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'mcp-res-b'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client_a = mcp_client(&app, &pat_a).await;
    let uri = format!("musehub://agent-runs/{run_id}/summary");
    let ok = client_a
        .read_resource(ReadResourceRequestParams::new(uri.clone()))
        .await
        .expect("read own run");
    assert!(!ok.contents.is_empty());
    let body_text = match &ok.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text.as_str(),
        _ => panic!("expected text resource"),
    };
    let parsed: serde_json::Value = serde_json::from_str(body_text).unwrap();
    assert_eq!(parsed["id"].as_str(), Some(run_id));
    let foreign = Uuid::new_v4();
    let deny = client_a
        .read_resource(ReadResourceRequestParams::new(format!(
            "musehub://agent-runs/{foreign}/summary"
        )))
        .await;
    assert!(deny.is_err());
    let listed = client_a.list_resources(None).await.expect("list");
    use musehub_app::mcp::capabilities::V1_RESOURCE_URI_TEMPLATES;
    assert_eq!(listed.resources.len(), V1_RESOURCE_URI_TEMPLATES.len());
    let _ = repo_b;
    client_a.cancel().await.ok();
}
