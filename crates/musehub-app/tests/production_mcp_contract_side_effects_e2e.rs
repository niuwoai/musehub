//! MCP §5–9 write tools: side effects, idempotency conflicts, and permission negatives.

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
    let v = result
        .structured_content
        .as_ref()
        .expect("structured")
        .clone();
    v.get("data").cloned().unwrap_or(v)
}

async fn admin_pat(app: &axum::Router, cookie: &str) -> String {
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(r#"{"name":"side-fx","org":"demo"}"#))
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

fn push_feature_branch(git_root: &std::path::Path, storage_key: Uuid, branch: &str) -> String {
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
    std::fs::write(work.join("pr-thread.txt"), b"thread\n").unwrap();
    assert!(Command::new("git")
        .current_dir(work)
        .args(["add", "pr-thread.txt"])
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
            "pr",
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
async fn mcp_discussion_comment_persists_row() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie).await;
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
                .body(Body::from(r#"{"slug":"side-fx","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'side-fx'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let client = mcp_client(&app, &token).await;
    let issue = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("idempotency_key".into(), json!("iss-side-fx")),
                    ("title".into(), json!("Discuss")),
                    ("body".into(), json!("")),
                ]),
            ),
        )
        .await
        .expect("issue");
    let issue_id = tool_data(&issue)["id"].as_str().unwrap().to_string();
    let comment = client
        .call_tool(
            CallToolRequestParams::new("musehub_discussion_comment").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("idempotency_key".into(), json!("disc-side-fx-v1")),
                    ("target".into(), json!({"kind": "issue", "id": issue_id})),
                    ("body".into(), json!("visible")),
                ]),
            ),
        )
        .await
        .expect("comment");
    assert!(!comment.is_error.unwrap_or(true));
    let comment_id = tool_data(&comment)["id"].as_str().unwrap().to_string();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM discussion_comments WHERE id = $1")
        .bind(Uuid::parse_str(&comment_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_issue_criteria_replace_increments_revision_and_rows() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie).await;
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
                .body(Body::from(r#"{"slug":"crit-fx","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'crit-fx'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let client = mcp_client(&app, &token).await;
    let issue = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("idempotency_key".into(), json!("iss-crit-fx-v1")),
                    ("title".into(), json!("Criteria")),
                    ("body".into(), json!("")),
                    ("criteria".into(), json!([])),
                ]),
            ),
        )
        .await
        .expect("issue");
    assert!(!issue.is_error.unwrap_or(true));
    let issue_id = tool_data(&issue)["id"].as_str().unwrap().to_string();
    let replaced = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_criteria_replace").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("issue_id".into(), json!(issue_id)),
                    ("expected_version".into(), json!(1)),
                    (
                        "criteria".into(),
                        json!([{
                            "description": "new criterion",
                            "verification_method": "manual",
                            "required": true
                        }]),
                    ),
                    ("idempotency_key".into(), json!("crit-replace-v1")),
                ]),
            ),
        )
        .await
        .expect("replace");
    assert!(!replaced.is_error.unwrap_or(true));
    assert_eq!(tool_data(&replaced)["version"], 2);
    let crit_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM issue_acceptance_criteria c
         JOIN issue_revisions r ON r.id = c.issue_revision_id
         WHERE r.issue_id = $1 AND r.revision = 2",
    )
    .bind(Uuid::parse_str(&issue_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(crit_count.0, 1);
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_plan_accept_persists_accepted_state() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie).await;
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
                .body(Body::from(r#"{"slug":"plan-acc","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'plan-acc'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let client = mcp_client(&app, &token).await;
    let issue = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("idempotency_key".into(), json!("iss-plan-acc")),
                    ("title".into(), json!("Plan")),
                    ("body".into(), json!("")),
                ]),
            ),
        )
        .await
        .expect("issue");
    let revision_id = tool_data(&issue)["current_revision_id"]
        .as_str()
        .unwrap()
        .to_string();
    let plan = client
        .call_tool(
            CallToolRequestParams::new("musehub_plan_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("issue_revision_id".into(), json!(revision_id)),
                    ("body".into(), json!("plan")),
                    ("steps".into(), json!([])),
                    ("idempotency_key".into(), json!("plan-acc-v1")),
                ]),
            ),
        )
        .await
        .expect("plan");
    let plan_id = tool_data(&plan)["id"].as_str().unwrap().to_string();
    let accepted = client
        .call_tool(
            CallToolRequestParams::new("musehub_plan_accept").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("plan_id".into(), json!(plan_id)),
                    ("idempotency_key".into(), json!("accept-plan-v1")),
                ]),
            ),
        )
        .await
        .expect("accept");
    assert!(!accepted.is_error.unwrap_or(true));
    let state: (String,) = sqlx::query_as("SELECT state FROM implementation_plans WHERE id = $1")
        .bind(Uuid::parse_str(&plan_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "accepted");
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_issue_create_idempotency_key_conflict() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie).await;
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
                .body(Body::from(r#"{"slug":"idem-iss","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'idem-iss'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let client = mcp_client(&app, &token).await;
    let key = "idem-key-matrix";
    let first = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("idempotency_key".into(), json!(key)),
                    ("title".into(), json!("one")),
                    ("body".into(), json!("")),
                ]),
            ),
        )
        .await
        .expect("first");
    assert!(!first.is_error.unwrap_or(true));
    let conflict = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("idempotency_key".into(), json!(key)),
                    ("title".into(), json!("two")),
                    ("body".into(), json!("")),
                ]),
            ),
        )
        .await;
    assert!(
        conflict.is_err()
            || conflict
                .as_ref()
                .ok()
                .and_then(|r| r.is_error)
                .unwrap_or(false),
        "idempotency conflict must fail"
    );
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM issues WHERE repo_id = $1")
        .bind(repo_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_release_create_persists_draft_row() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie).await;
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
                .body(Body::from(r#"{"slug":"rel-fx","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'rel-fx'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let oid = "d".repeat(40);
    let pipe_id = Uuid::new_v4();
    let artifact_bytes = b"rel-artifact";
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
    let client = mcp_client(&app, &token).await;
    let rel = client
        .call_tool(
            CallToolRequestParams::new("musehub_release_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("version".into(), json!("0.1.0-side")),
                    ("tag_name".into(), json!("v0.1.0-side")),
                    ("commit_oid".into(), json!(oid)),
                    ("pipeline_run_id".into(), json!(pipe_id.to_string())),
                    ("idempotency_key".into(), json!("rel-create-v1")),
                ]),
            ),
        )
        .await
        .expect("release");
    assert!(!rel.is_error.unwrap_or(true));
    let rel_id = tool_data(&rel)["id"].as_str().unwrap().to_string();
    let row: (String,) = sqlx::query_as("SELECT state FROM releases WHERE id = $1")
        .bind(Uuid::parse_str(&rel_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "draft");
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_discussion_resolve_updates_pr_comment() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie).await;
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client = mcp_client(&app, &token).await;
    let slug = format!("resolve-pr-{}", Uuid::new_v4().simple());
    let repo = client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("slug".into(), json!(slug.clone())),
                    ("idempotency_key".into(), json!("resolve-repo-v1")),
                ]),
            ),
        )
        .await
        .expect("repo");
    let repo_id = tool_data(&repo)["id"].as_str().unwrap().to_string();
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(Uuid::parse_str(&repo_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    let head = push_feature_branch(git_root.path(), storage_key.0, "feature/resolve");
    let pr = client
        .call_tool(
            CallToolRequestParams::new("musehub_pr_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("title".into(), json!("Resolve thread")),
                    ("body".into(), json!("")),
                    ("source_branch".into(), json!("feature/resolve")),
                    ("target_branch".into(), json!("main")),
                    ("expected_head_oid".into(), json!(head)),
                    ("issue_revision_ids".into(), json!([])),
                    ("draft".into(), json!(false)),
                    ("idempotency_key".into(), json!("pr-resolve-v1")),
                ]),
            ),
        )
        .await
        .expect("pr");
    assert!(!pr.is_error.unwrap_or(true));
    let pr_id = tool_data(&pr)["id"].as_str().unwrap().to_string();
    let comment = client
        .call_tool(
            CallToolRequestParams::new("musehub_discussion_comment").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("idempotency_key".into(), json!("pr-comment-v1")),
                    ("target".into(), json!({"kind": "pr", "id": pr_id})),
                    ("body".into(), json!("fix this")),
                ]),
            ),
        )
        .await
        .expect("comment");
    let comment_id = tool_data(&comment)["id"].as_str().unwrap().to_string();
    let row_version: (i64,) = sqlx::query_as(
        "SELECT extract(epoch from updated_at)::bigint FROM discussion_comments WHERE id = $1",
    )
    .bind(Uuid::parse_str(&comment_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let resolved = client
        .call_tool(
            CallToolRequestParams::new("musehub_discussion_resolve").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("comment_id".into(), json!(comment_id)),
                    ("expected_version".into(), json!(row_version.0)),
                    ("resolved".into(), json!(true)),
                    ("idempotency_key".into(), json!("resolve-thread-v1")),
                ]),
            ),
        )
        .await
        .expect("resolve");
    assert!(!resolved.is_error.unwrap_or(true));
    let flag: (bool,) = sqlx::query_as("SELECT resolved FROM discussion_comments WHERE id = $1")
        .bind(Uuid::parse_str(&comment_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(flag.0);
    client.cancel().await.ok();
}
