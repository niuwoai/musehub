//! MCP: selected PAT same org, second repo denied (resource read + write tools).

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use http_body_util::BodyExt;
use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams};
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

#[tokio::test]
async fn selected_pat_same_org_other_repo_denied_for_resources_and_writes() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    for slug in ["sel-a", "sel-b"] {
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
    let repo_a: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM repositories WHERE slug = 'sel-a'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let repo_b: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM repositories WHERE slug = 'sel-b'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;

    let issue_b: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/sel-b/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"title":"b"}"#))
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
    let issue_b_id = issue_b["id"].as_str().unwrap();

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
                        "name": "sel-a-only",
                        "org": "demo",
                        "scopes": ["repo:read", "repo:write"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_a]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_body: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = pat_body["token"].as_str().unwrap();

    let client = mcp_client(&app, token).await;
    let deny_issue = client
        .read_resource(ReadResourceRequestParams::new(format!(
            "musehub://issues/{issue_b_id}/revisions/1"
        )))
        .await;
    assert!(deny_issue.is_err());

    let oid = "b".repeat(40);
    let deny_file = client
        .read_resource(ReadResourceRequestParams::new(format!(
            "musehub://repositories/{repo_b}/commits/{oid}/files/README.md"
        )))
        .await;
    assert!(deny_file.is_err());

    let org_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let mut args = serde_json::Map::new();
    args.insert("org_id".into(), json!(org_id.to_string()));
    args.insert("repository_id".into(), json!(repo_b.to_string()));
    args.insert("title".into(), json!("x"));
    args.insert("idempotency_key".into(), json!("sel-b-deny-write"));
    let deny_write = client
        .call_tool(CallToolRequestParams::new("musehub_issue_create").with_arguments(args))
        .await
        .expect("call");
    assert!(deny_write.is_error.unwrap_or(false));
    client.cancel().await.ok();
}
