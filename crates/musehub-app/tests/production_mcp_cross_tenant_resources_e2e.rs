//! Cross-tenant: PAT bound to org B cannot read org A agent-run resource.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use http_body_util::BodyExt;
use rmcp::model::ReadResourceRequestParams;
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
async fn other_org_pat_cannot_read_demo_agent_run_summary() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    let other_org = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, slug, slug_normalized, display_name, status)
         VALUES ($1,'acme','acme','Acme Inc','active')",
    )
    .bind(other_org)
    .execute(&pool)
    .await
    .unwrap();
    let admin_id: (Uuid,) = sqlx::query_as("SELECT id FROM principals WHERE handle = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status)
         VALUES ($1,$2,$3,'admin','active')",
    )
    .bind(Uuid::new_v4())
    .bind(other_org)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"tenant-a","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/tenant-a/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"title":"x"}"#))
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
                .uri(format!("/api/v1/orgs/demo/repos/tenant-a/issues/{n}/plans"))
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
                .uri(format!("/api/v1/orgs/demo/repos/tenant-a/issues/{n}/plans"))
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
                    .uri("/api/v1/orgs/demo/repos/tenant-a/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        json!({ "issue_id": issue["id"], "message": "hi" }).to_string(),
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
    for _ in 0..80 {
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
        if matches!(
            st["state"].as_str(),
            Some("succeeded") | Some("failed") | Some("canceled")
        ) {
            break;
        }
    }
    let pat_other = {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tokens")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"name":"acme-only","org":"acme"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        v["token"].as_str().unwrap().to_string()
    };
    let client = mcp_client(&app, &pat_other).await;
    let uri = format!("musehub://agent-runs/{run_id}/summary");
    let denied = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await;
    assert!(
        denied.is_err(),
        "cross-org PAT must not read demo agent run"
    );
    client.cancel().await.ok();
}
