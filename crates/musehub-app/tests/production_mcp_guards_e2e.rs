//! MCP guards: tampered publish/deploy, failed pipeline release block, no stray side effects.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
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

#[tokio::test]
async fn mcp_deploy_unpublished_release_creates_no_deployment() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"mcp-guard-dep","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat = {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tokens")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"name":"g","org":"demo"}"#))
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
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug = 'mcp-guard-dep'")
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
    let oid = "f".repeat(40);
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
         VALUES ($1,$2,$3,'0.0.2','v0.0.2',$4,$5,'draft')",
    )
    .bind(release_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(pipe_id)
    .execute(&pool)
    .await
    .unwrap();
    let client = mcp_client(&app, &pat).await;
    let bad = client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_start").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("environment_id".into(), json!(env_id.0.to_string())),
                    ("release_id".into(), json!(release_id.to_string())),
                    ("idempotency_key".into(), json!("dep-unpub-guard-v1")),
                ]),
            ),
        )
        .await
        .expect("call");
    assert!(bad.is_error.unwrap_or(true));
    let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM deployments WHERE release_id = $1")
        .bind(release_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n.0, 0);
    client.cancel().await.ok();
}

#[tokio::test]
async fn api_publish_tampered_oci_stays_draft() {
    std::env::set_var("MUSEHUB_REQUIRE_PIPELINE_OCI", "0");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let admin = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin)
                .body(Body::from(r#"{"slug":"mcp-guard-oci","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug = 'mcp-guard-oci'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let principal: (Uuid,) = sqlx::query_as("SELECT id FROM principals WHERE handle = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let pipe_id = Uuid::new_v4();
    let oid = "a".repeat(40);
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
    let art_id = Uuid::new_v4();
    let good = format!(
        "127.0.0.1:5000/musehub/pipeline-{}@sha256:{}",
        repo_id.0,
        "e".repeat(64)
    );
    sqlx::query(
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, kind, sha256, byte_length, storage_path, state, oci_image_ref)
         VALUES ($1,$2,$3,$4,'oci-image.ref','oci_image',$5,10,'/tmp','ready',$6)",
    )
    .bind(art_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(pipe_id)
    .bind("e".repeat(64))
    .bind(&good)
    .execute(&pool)
    .await
    .unwrap();
    let release_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state)
         VALUES ($1,$2,$3,'0.0.3','v0.0.3',$4,$5,'draft')",
    )
    .bind(release_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(pipe_id)
    .execute(&pool)
    .await
    .unwrap();
    let tampered = format!(
        "127.0.0.1:5000/musehub/pipeline-{}@sha256:{}",
        repo_id.0,
        "d".repeat(64)
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/mcp-guard-oci/releases/{release_id}/publish"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin)
                .body(Body::from(json!({ "oci_image_ref": tampered }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let state: (String,) = sqlx::query_as("SELECT state FROM releases WHERE id = $1")
        .bind(release_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "draft");
    let _ = art_id;
}

#[tokio::test]
async fn release_create_rejects_failed_pipeline() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let admin = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin)
                .body(Body::from(r#"{"slug":"pipe-fail-rel","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug = 'pipe-fail-rel'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let principal: (Uuid,) = sqlx::query_as("SELECT id FROM principals WHERE handle = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let pipe_id = Uuid::new_v4();
    let oid = "b".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','failed',$5,$6,'release_build')",
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
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pipe-fail-rel/releases")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin)
                .body(Body::from(
                    json!({
                        "version": "0.0.4",
                        "tag_name": "v0.0.4",
                        "commit_oid": oid,
                        "pipeline_run_id": pipe_id.to_string(),
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM releases WHERE repo_id = $1")
        .bind(repo_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n.0, 0);
}
