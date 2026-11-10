//! MCP Resources §13: full V1 catalog reads (deployment, issue revision) + cross-tenant deny.

mod common;

use std::collections::HashMap;

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
use sha2::{Digest, Sha256};
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
async fn mcp_v1_resource_reads_match_capabilities_catalog() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"res-v1","org":"demo"}"#))
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
    let token = pat_json["token"].as_str().unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"res-v1-cat","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'res-v1-cat'")
            .fetch_one(&pool)
            .await
            .unwrap();
    musehub_app::seed_repo_environment(&pool, org_id.0, repo_id.0)
        .await
        .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/res-v1-cat/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"title":"Res","body":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(issue_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let issue_id = issue_json["id"].as_str().unwrap();
    let manifest_sha = "c".repeat(64);
    let oid = "e".repeat(40);
    let pipe_id = Uuid::new_v4();
    let artifact_bytes = b"res-bundle";
    let sha_hex: String = Sha256::digest(artifact_bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    let artifact_dir = git_root
        .path()
        .join("artifacts")
        .join(org_id.0.to_string())
        .join(repo_id.0.to_string());
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("bundle.tar"), artifact_bytes).unwrap();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'release','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id.0)
    .bind(repo_id.0)
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
    .bind(org_id.0)
    .bind(repo_id.0)
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
         VALUES ($1,$2,$3,'1','v1',$4,$5,'published',$6)",
    )
    .bind(release_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(pipe_id)
    .bind(&manifest_sha)
    .execute(&pool)
    .await
    .unwrap();
    let test_env: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'test'")
            .bind(repo_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let deployment_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id)
         VALUES ($1,$2,$3,$4,$5,'compose-mock','succeeded',$6)",
    )
    .bind(deployment_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(test_env.0)
    .bind(release_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();

    let client = mcp_client(&app, &token).await;
    use musehub_app::mcp::capabilities::{
        V1_RESOURCE_READS_IMPLEMENTED, V1_RESOURCE_URI_TEMPLATES,
    };
    let listed = client.list_resources(None).await.expect("list");
    assert_eq!(listed.resources.len(), V1_RESOURCE_URI_TEMPLATES.len());

    let issue_uri = format!("musehub://issues/{issue_id}/revisions/1");
    let issue_res = client
        .read_resource(ReadResourceRequestParams::new(issue_uri.clone()))
        .await
        .expect("issue revision");
    let issue_text = match &issue_res.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text.clone(),
        _ => panic!("text"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&issue_text).unwrap();
    assert_eq!(parsed["issue_id"].as_str(), Some(issue_id));

    let dep_uri = format!("musehub://deployments/{deployment_id}");
    let dep_res = client
        .read_resource(ReadResourceRequestParams::new(dep_uri))
        .await
        .expect("deployment");
    let dep_text = match &dep_res.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text.clone(),
        _ => panic!("text"),
    };
    let dep_json: serde_json::Value = serde_json::from_str(&dep_text).unwrap();
    assert_eq!(dep_json["state"], "succeeded");

    let foreign = Uuid::new_v4();
    let deny = client
        .read_resource(ReadResourceRequestParams::new(format!(
            "musehub://deployments/{foreign}"
        )))
        .await;
    assert!(deny.is_err());

    let caps = client
        .call_tool(rmcp::model::CallToolRequestParams::new(
            "musehub_capabilities_get",
        ))
        .await
        .expect("caps");
    let reads = caps
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("resource_reads_implemented_v1"))
        .and_then(|v| v.as_array())
        .expect("reads");
    assert_eq!(reads.len(), V1_RESOURCE_READS_IMPLEMENTED.len());
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_v1_resource_reads_denied_for_other_org_pat() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let other_org = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, slug, slug_normalized, display_name, status)
         VALUES ($1,'acme-res','acme-res','Acme','active')",
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
                .body(Body::from(r#"{"slug":"res-cross-org","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'res-cross-org'")
            .fetch_one(&pool)
            .await
            .unwrap();
    musehub_app::seed_repo_environment(&pool, org_id.0, repo_id.0)
        .await
        .unwrap();
    let issue_json: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/res-cross-org/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"title":"x","body":""}"#))
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
    let issue_id = issue_json["id"].as_str().unwrap();
    let agent_json: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/res-cross-org/issues/1/plans")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"body":"p"}"#))
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
    let _ = agent_json;
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/orgs/demo/repos/res-cross-org/issues/1/plans")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let run_json: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/res-cross-org/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        json!({ "issue_id": issue_id, "message": "go" }).to_string(),
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
    let run_id = run_json["id"].as_str().unwrap();
    let dep_id = Uuid::new_v4();
    let rel_id = Uuid::new_v4();
    let pipe_id = Uuid::new_v4();
    let env_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'test'")
            .bind(repo_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'ci','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind("b".repeat(40))
    .bind(admin_id.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state)
         VALUES ($1,$2,$3,'1','v1',$4,$5,'published')",
    )
    .bind(rel_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind("b".repeat(40))
    .bind(pipe_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id)
         VALUES ($1,$2,$3,$4,$5,'compose-mock','succeeded',$6)",
    )
    .bind(dep_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(env_id.0)
    .bind(rel_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let pr_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pull_requests (id, org_id, repo_id, number, author_id, title, source_branch, target_branch, state, row_version)
         VALUES ($1,$2,$3,1,$4,'pr','feature','main','open',2)",
    )
    .bind(pr_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();
    let pat_other = {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tokens")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"name":"acme-only","org":"acme-res"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(
            &axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let client = mcp_client(&app, &pat_other).await;
    for uri in [
        format!("musehub://issues/{issue_id}/revisions/1"),
        format!("musehub://deployments/{dep_id}"),
        format!("musehub://pull-requests/{pr_id}/revisions/2"),
        format!("musehub://agent-runs/{run_id}/summary"),
    ] {
        let denied = client
            .read_resource(ReadResourceRequestParams::new(uri))
            .await;
        assert!(denied.is_err(), "expected cross-org deny");
    }
    client.cancel().await.ok();
}
