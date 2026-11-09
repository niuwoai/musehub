//! MCP write idempotency tools: remaining side effects (publish, review, cancel, retry, rollback).

mod common;

use std::collections::HashMap;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use musehub_app::bare_repo_path;
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
    let v = result.structured_content.as_ref().expect("structured").clone();
    v.get("data").cloned().unwrap_or(v)
}

async fn admin_pat(app: &axum::Router, cookie: &str, name: &str) -> String {
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

fn push_branch(git_root: &std::path::Path, storage_key: Uuid, branch: &str) -> String {
    let bare = bare_repo_path(git_root, &storage_key);
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    assert!(Command::new("git")
        .args(["clone", bare.to_str().unwrap(), work.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(work)
        .args(["checkout", "-b", branch])
        .status()
        .unwrap()
        .success());
    std::fs::write(work.join("x.txt"), b"x").unwrap();
    assert!(Command::new("git")
        .current_dir(work)
        .args(["add", "x.txt"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(work)
        .args([
            "-c",
            "user.email=itest@musehub.local",
            "-c",
            "user.name=itest",
            "commit",
            "-m",
            "x",
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(work)
        .args(["push", "origin", branch])
        .status()
        .unwrap()
        .success());
    let out = Command::new("git")
        .current_dir(work)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn mcp_release_publish_sets_published_state() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie, "rel-pub").await;
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
                .body(Body::from(r#"{"slug":"rel-pub","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'rel-pub'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let oid = "f".repeat(40);
    let pipe_id = Uuid::new_v4();
    let bytes = b"b";
    let sha_hex: String = Sha256::digest(bytes).iter().map(|b| format!("{:02x}", b)).collect();
    let dir = git_root.path().join("artifacts").join(org_id.0.to_string()).join(repo_id.0.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bundle.tar"), bytes).unwrap();
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
    .bind(bytes.len() as i64)
    .bind(dir.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .unwrap();
    let client = mcp_client(&app, &token).await;
    let created = client
        .call_tool(CallToolRequestParams::new("musehub_release_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("version".into(), json!("1.0.1")),
                ("tag_name".into(), json!("v1.0.1")),
                ("commit_oid".into(), json!(oid)),
                ("pipeline_run_id".into(), json!(pipe_id.to_string())),
                ("idempotency_key".into(), json!("rel-create-pub")),
            ]),
        ))
        .await
        .expect("create");
    assert!(!created.is_error.unwrap_or(true));
    let rel_id = tool_data(&created)["id"].as_str().unwrap().to_string();
    let manifest_hash = tool_data(&created)["manifest_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let published = client
        .call_tool(
            CallToolRequestParams::new("musehub_release_publish").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("release_id".into(), json!(rel_id)),
                    ("expected_commit_oid".into(), json!(oid)),
                    ("expected_manifest_hash".into(), json!(manifest_hash)),
                    ("idempotency_key".into(), json!("rel-publish-v1")),
                ]),
            ),
        )
        .await
        .expect("publish");
    assert!(!published.is_error.unwrap_or(true));
    let state: (String,) = sqlx::query_as("SELECT state FROM releases WHERE id = $1")
        .bind(Uuid::parse_str(&rel_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "published");
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_pr_review_and_update_side_effects() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie, "pr-wr").await;
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client = mcp_client(&app, &token).await;
    let slug = format!("pr-wr-{}", Uuid::new_v4().simple());
    let repo = client
        .call_tool(CallToolRequestParams::new("musehub_repository_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("slug".into(), json!(slug)),
                ("idempotency_key".into(), json!("pr-wr-repo")),
            ]),
        ))
        .await
        .expect("repo");
    let repo_id = tool_data(&repo)["id"].as_str().unwrap().to_string();
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(Uuid::parse_str(&repo_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    let head = push_branch(git_root.path(), storage_key.0, "feature/wr");
    let pr = client
        .call_tool(CallToolRequestParams::new("musehub_pr_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id)),
                ("title".into(), json!("Before")),
                ("body".into(), json!("")),
                ("source_branch".into(), json!("feature/wr")),
                ("target_branch".into(), json!("main")),
                ("expected_head_oid".into(), json!(head)),
                ("issue_revision_ids".into(), json!([])),
                ("draft".into(), json!(false)),
                ("idempotency_key".into(), json!("pr-wr-create")),
            ]),
        ))
        .await
        .expect("pr");
    let pr_id = tool_data(&pr)["id"].as_str().unwrap().to_string();
    let row_version: (i64,) =
        sqlx::query_as("SELECT row_version FROM pull_requests WHERE id = $1")
            .bind(Uuid::parse_str(&pr_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    let updated = client
        .call_tool(CallToolRequestParams::new("musehub_pr_update").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id)),
                ("pr_id".into(), json!(pr_id)),
                ("expected_version".into(), json!(row_version.0)),
                ("title".into(), json!("After")),
                ("idempotency_key".into(), json!("pr-update-v1")),
            ]),
        ))
        .await
        .expect("update");
    assert!(!updated.is_error.unwrap_or(true));
    let title: (String,) = sqlx::query_as("SELECT title FROM pull_requests WHERE id = $1")
        .bind(Uuid::parse_str(&pr_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title.0, "After");
    let review = client
        .call_tool(CallToolRequestParams::new("musehub_pr_review").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id)),
                ("pr_id".into(), json!(pr_id)),
                ("decision".into(), json!("comment")),
                ("body".into(), json!("looks fine")),
                ("idempotency_key".into(), json!("pr-review-v1")),
            ]),
        ))
        .await
        .expect("review");
    assert!(!review.is_error.unwrap_or(true));
    let reviews: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pr_reviews WHERE pr_id = $1")
            .bind(Uuid::parse_str(&pr_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reviews.0, 1);
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_pipeline_cancel_and_job_retry() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie, "pipe-job").await;
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
                .body(Body::from(r#"{"slug":"pipe-job","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'pipe-job'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let run_queued = Uuid::new_v4();
    let run_failed = Uuid::new_v4();
    let oid = "a".repeat(40);
    for (rid, st) in [(run_queued, "running"), (run_failed, "failed")] {
        sqlx::query(
            "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
             VALUES ($1,$2,$3,$4,'ci',$5,$6,$7,'release_build')",
        )
        .bind(rid)
        .bind(org_id.0)
        .bind(repo_id.0)
        .bind(&oid)
        .bind(st)
        .bind(admin_id.0)
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
    }
    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','failed')",
    )
    .bind(job_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(run_failed)
    .execute(&pool)
    .await
    .unwrap();
    let client = mcp_client(&app, &token).await;
    let canceled = client
        .call_tool(
            CallToolRequestParams::new("musehub_pipeline_cancel").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("pipeline_run_id".into(), json!(run_queued.to_string())),
                    ("reason".into(), json!("stop")),
                    ("idempotency_key".into(), json!("pipe-cancel-v1")),
                ]),
            ),
        )
        .await
        .expect("cancel");
    assert!(!canceled.is_error.unwrap_or(true));
    let pipe_state: (String,) = sqlx::query_as("SELECT state FROM pipeline_runs WHERE id = $1")
        .bind(run_queued)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pipe_state.0, "cancel_requested");
    let retried = client
        .call_tool(CallToolRequestParams::new("musehub_job_retry").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("job_id".into(), json!(job_id.to_string())),
                ("idempotency_key".into(), json!("job-retry-v1")),
            ]),
        ))
        .await
        .expect("retry");
    assert!(!retried.is_error.unwrap_or(true));
    let job_state: (String,) = sqlx::query_as("SELECT state FROM pipeline_jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(job_state.0, "queued");
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_agent_run_cancel_requested_by_pat_principal() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie, "ag-cancel").await;
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
                .body(Body::from(r#"{"slug":"ag-cancel","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'ag-cancel'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let issue_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, author_id, title, body, state)
         VALUES ($1,$2,$3,1,$4,'i','','open')",
    )
    .bind(issue_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, requested_by, adapter_kind, work_branch, state)
         VALUES ($1,$2,$3,$4,$5,'reference','agent/test','queued')",
    )
    .bind(run_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(issue_id)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();
    let client = mcp_client(&app, &token).await;
    let canceled = client
        .call_tool(
            CallToolRequestParams::new("musehub_agent_run_cancel").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("agent_run_id".into(), json!(run_id.to_string())),
                    ("idempotency_key".into(), json!("agent-cancel-v1")),
                ]),
            ),
        )
        .await
        .expect("cancel");
    assert!(!canceled.is_error.unwrap_or(true));
    let state: (String,) = sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "cancel_requested");
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_deployment_rollback_inserts_new_row() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie, "dep-rb").await;
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
                .body(Body::from(r#"{"slug":"dep-rb","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'dep-rb'")
            .fetch_one(&pool)
            .await
            .unwrap();
    musehub_app::seed_repo_environment(&pool, org_id.0, repo_id.0)
        .await
        .unwrap();
    let env_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'test'")
            .bind(repo_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let rel_id = Uuid::new_v4();
    let pipe_id = Uuid::new_v4();
    let oid = "c".repeat(40);
    let manifest = "d".repeat(64);
    let bytes = b"rb";
    let sha_hex: String = Sha256::digest(bytes).iter().map(|b| format!("{:02x}", b)).collect();
    let dir = git_root.path().join("artifacts").join(org_id.0.to_string()).join(repo_id.0.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bundle.tar"), bytes).unwrap();
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
    .bind(bytes.len() as i64)
    .bind(dir.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'1','v1',$4,$5,'published',$6)",
    )
    .bind(rel_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(pipe_id)
    .bind(&manifest)
    .execute(&pool)
    .await
    .unwrap();
    let target_dep = Uuid::new_v4();
    let health = serde_json::json!({
        "status": "healthy",
        "compose_project": format!("musehub-deploy-{target_dep}"),
        "adapter": "compose-mock",
    });
    sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id, plan_hash, requested_by, health_summary)
         VALUES ($1,$2,$3,$4,$5,'compose-mock','succeeded',$6,$7,$8,$9)",
    )
    .bind(target_dep)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(env_id.0)
    .bind(rel_id)
    .bind(Uuid::new_v4())
    .bind(&manifest)
    .bind(admin_id.0)
    .bind(health)
    .execute(&pool)
    .await
    .unwrap();
    let before: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM deployments WHERE repo_id = $1")
            .bind(repo_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let client = mcp_client(&app, &token).await;
    let rollback = client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_rollback").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("environment_id".into(), json!(env_id.0.to_string())),
                    ("target_healthy_deployment_id".into(), json!(target_dep.to_string())),
                    ("reason".into(), json!("mcp rb")),
                    ("idempotency_key".into(), json!("dep-rollback-v1")),
                ]),
            ),
        )
        .await
        .expect("rollback");
    assert!(!rollback.is_error.unwrap_or(true));
    let after: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM deployments WHERE repo_id = $1")
            .bind(repo_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after.0, before.0 + 1);
    client.cancel().await.ok();
}
