//! MCP Resources §13: cross-repo / missing grant read denials.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
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
async fn resource_reads_denied_without_repo_grant() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    for slug in ["res-grant-a", "res-grant-b"] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(format!(
                        r#"{{"slug":"{slug}","description":""}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let (org_id, repo_a, repo_b): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT o.id, ra.id, rb.id FROM organizations o
         JOIN repositories ra ON ra.org_id = o.id AND ra.slug_normalized = 'res-grant-a'
         JOIN repositories rb ON rb.org_id = o.id AND rb.slug_normalized = 'res-grant-b'
         WHERE o.slug = 'demo'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let issue_b = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/res-grant-b/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"title":"secret","body":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(issue_b.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let issue_id = issue_json["id"].as_str().unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let dep_b = Uuid::new_v4();
    let rel_b = Uuid::new_v4();
    let env_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'test'")
            .bind(repo_b)
            .fetch_one(&pool)
            .await
            .unwrap();
    let env_id = env_id.0;
    let pipe_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'ci','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_b)
    .bind(org_id)
    .bind(repo_b)
    .bind("a".repeat(40))
    .bind(admin_id.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state)
         VALUES ($1,$2,$3,'1','v1',$4,$5,'published')",
    )
    .bind(rel_b)
    .bind(org_id)
    .bind(repo_b)
    .bind("a".repeat(40))
    .bind(pipe_b)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id)
         VALUES ($1,$2,$3,$4,$5,'compose-mock','succeeded',$6)",
    )
    .bind(dep_b)
    .bind(org_id)
    .bind(repo_b)
    .bind(env_id)
    .bind(rel_b)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let pr_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pull_requests (id, org_id, repo_id, number, author_id, title, source_branch, target_branch, state, row_version)
         VALUES ($1,$2,$3,1,$4,'pr','feature','main','open',3)",
    )
    .bind(pr_b)
    .bind(org_id)
    .bind(repo_b)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();
    let member_id = Uuid::new_v4();
    let cred_id = Uuid::new_v4();
    let pass = "Res-Grant-Member-22!";
    let hash = musehub_app::hash_password_for_tests(pass).unwrap();
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1,'human','resreader','resreader','Reader','active')",
    )
    .bind(member_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_credentials (id, principal_id, password_hash, password_changed_at)
         VALUES ($1,$2,$3,CURRENT_TIMESTAMP)",
    )
    .bind(cred_id)
    .bind(member_id)
    .bind(hash)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status) VALUES ($1,$2,$3,'member','active')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(member_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1,$2,$3,$4,'developer')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_a)
    .bind(member_id)
    .execute(&pool)
    .await
    .unwrap();
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"handle": "resreader", "password": pass}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let member_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &member_cookie)
                .body(Body::from(
                    json!({
                        "name": "res-reader",
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
    let token = serde_json::from_slice::<serde_json::Value>(
        &axum::body::to_bytes(pat_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let client = mcp_client(&app, &token).await;
    let issue_uri = format!("musehub://issues/{issue_id}/revisions/1");
    assert!(client
        .read_resource(ReadResourceRequestParams::new(issue_uri))
        .await
        .is_err());
    assert!(client
        .read_resource(ReadResourceRequestParams::new(format!(
            "musehub://deployments/{dep_b}"
        )))
        .await
        .is_err());
    assert!(client
        .read_resource(ReadResourceRequestParams::new(format!(
            "musehub://pull-requests/{pr_b}/revisions/3"
        )))
        .await
        .is_err());
    client.cancel().await.ok();
}
