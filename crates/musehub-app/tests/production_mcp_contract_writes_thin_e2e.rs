//! MCP write tools with thin matrix coverage: pr_merge, agent_run_respond, pipeline_run idempotency.

mod common;

use std::collections::HashMap;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
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
use tokio::net::TcpListener;
use tower::ServiceExt as TowerServiceExt;
use uuid::Uuid;

const DEFAULT_REPO_CHECK: &str = "product-ci";
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
    std::fs::write(work.join("merge.txt"), b"m").unwrap();
    assert!(Command::new("git")
        .current_dir(work)
        .args(["add", "merge.txt"])
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
            "m",
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
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

async fn setup_repo_pr(
    app: &axum::Router,
    pool: &sqlx::PgPool,
    git_root: &std::path::Path,
    cookie: &str,
    admin_token: &str,
    slug: &str,
    reviewer_handle: &str,
    reviewer_pass: &str,
) -> (Uuid, Uuid, String, String, String) {
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(pool)
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(
                    json!({
                        "handle": reviewer_handle,
                        "password": reviewer_pass,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(format!(
                    r#"{{"slug":"{slug}","description":""}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{slug}/collaborators"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(format!(
                    r#"{{"handle":"{reviewer_handle}","role":"maintainer"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = $1")
            .bind(slug)
            .fetch_one(pool)
            .await
            .unwrap();
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(repo_id.0)
        .fetch_one(pool)
        .await
        .unwrap();
    let head = push_branch(git_root, storage_key.0, "feature/thin-merge");
    let client = mcp_client(app, admin_token).await;
    let pr = client
        .call_tool(CallToolRequestParams::new("musehub_pr_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("title".into(), json!("Thin merge")),
                ("body".into(), json!("")),
                ("source_branch".into(), json!("feature/thin-merge")),
                ("target_branch".into(), json!("main")),
                ("expected_head_oid".into(), json!(head.clone())),
                ("issue_revision_ids".into(), json!([])),
                ("draft".into(), json!(false)),
                ("idempotency_key".into(), json!("thin-pr-create")),
            ]),
        ))
        .await
        .expect("pr_create");
    assert!(!pr.is_error.unwrap_or(true));
    let pr_id = tool_data(&pr)["id"].as_str().unwrap().to_string();
    let oids: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT head_oid, base_oid FROM pull_requests WHERE id = $1",
    )
    .bind(Uuid::parse_str(&pr_id).unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    let head_oid = oids.0.unwrap();
    let base_oid = oids.1.unwrap();
    client.cancel().await.ok();
    (org_id.0, repo_id.0, pr_id, head_oid, base_oid)
}

#[tokio::test]
async fn mcp_pr_merge_rejects_without_approval() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let slug = format!("pr-merge-deny-{}", Uuid::new_v4().simple());
    let reviewer = format!("rev-deny-{}", &slug[..8]);
    let reviewer_pass = "Thin-Deny-Reviewer-12!";
    let token = admin_pat(&app, &cookie, "pr-merge-deny").await;
    let (org_id, repo_id, pr_id, head_oid, base_oid) = setup_repo_pr(
        &app,
        &pool,
        git_root.path(),
        &cookie,
        &token,
        &slug,
        &reviewer,
        reviewer_pass,
    )
    .await;
    let client = mcp_client(&app, &token).await;
    let denied = client
        .call_tool(CallToolRequestParams::new("musehub_pr_merge").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.to_string())),
                ("repository_id".into(), json!(repo_id.to_string())),
                ("pr_id".into(), json!(pr_id)),
                ("expected_head_oid".into(), json!(head_oid)),
                ("expected_base_oid".into(), json!(base_oid)),
                ("expected_candidate_oid".into(), json!(head_oid)),
                ("idempotency_key".into(), json!("pr-merge-early")),
            ]),
        ))
        .await
        .expect("merge call");
    assert!(denied.is_error.unwrap_or(true));
    let pr_state: (String,) = sqlx::query_as("SELECT state FROM pull_requests WHERE repo_id = $1")
        .bind(repo_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pr_state.0, "open");
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_pr_merge_succeeds_after_gates() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let slug = format!("pr-merge-ok-{}", Uuid::new_v4().simple());
    let reviewer = format!("rev-ok-{}", &slug[..8]);
    let reviewer_pass = "Thin-Ok-Reviewer-12!";
    let admin_token = admin_pat(&app, &cookie, "pr-merge-ok-admin").await;
    let (org_id, repo_id, pr_id, head_oid, base_oid) = setup_repo_pr(
        &app,
        &pool,
        git_root.path(),
        &cookie,
        &admin_token,
        &slug,
        &reviewer,
        reviewer_pass,
    )
    .await;
    let pr_number: (i64,) = sqlx::query_as("SELECT number FROM pull_requests WHERE id = $1")
        .bind(Uuid::parse_str(&pr_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    let reviewer_token = {
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"handle": reviewer, "password": reviewer_pass}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let reviewer_cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        admin_pat(&app, &reviewer_cookie, "reviewer-merge").await
    };
    let reviewer_client = mcp_client(&app, &reviewer_token).await;
    let approve = reviewer_client
        .call_tool(CallToolRequestParams::new("musehub_pr_review").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.to_string())),
                ("repository_id".into(), json!(repo_id.to_string())),
                ("pr_id".into(), json!(pr_id.clone())),
                ("decision".into(), json!("approve")),
                ("body".into(), json!("lgtm")),
                ("idempotency_key".into(), json!("pr-review-approve")),
            ]),
        ))
        .await
        .expect("review");
    assert!(!approve.is_error.unwrap_or(true));
    let ci_pass = "Thin-Ci-Reporter-12!";
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    json!({
                        "handle": "thin-ci",
                        "password": ci_pass,
                        "role": "admin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let ci_cookie = common::login_cookie(&app, "thin-ci", ci_pass).await;
    let check = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{slug}/pull-requests/{}/checks/{DEFAULT_REPO_CHECK}",
                    pr_number.0
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    json!({
                        "state": "success",
                        "attempt_id": Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(check.status(), StatusCode::NO_CONTENT);
    let merged = reviewer_client
        .call_tool(CallToolRequestParams::new("musehub_pr_merge").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.to_string())),
                ("repository_id".into(), json!(repo_id.to_string())),
                ("pr_id".into(), json!(pr_id)),
                ("expected_head_oid".into(), json!(head_oid)),
                ("expected_base_oid".into(), json!(base_oid)),
                ("expected_candidate_oid".into(), json!(head_oid)),
                ("idempotency_key".into(), json!("pr-merge-ok")),
            ]),
        ))
        .await
        .expect("merge");
    assert!(!merged.is_error.unwrap_or(true));
    let merged_data = tool_data(&merged);
    let merge_oid = merged_data["merge_commit_oid"]
        .as_str()
        .expect("merge_commit_oid")
        .to_string();
    let db_state: (String, Option<String>) = sqlx::query_as(
        "SELECT state, merged_oid FROM pull_requests WHERE repo_id = $1",
    )
    .bind(repo_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(db_state.0, "merged");
    assert_eq!(db_state.1.as_deref(), Some(merge_oid.as_str()));
    reviewer_client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_agent_run_respond_message_and_idempotency_conflict() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie, "ag-respond").await;
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
                .body(Body::from(r#"{"slug":"ag-respond","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'ag-respond'")
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
         VALUES ($1,$2,$3,$4,$5,'reference','agent/wait','waiting_input')",
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
    let wrong_state = client
        .call_tool(CallToolRequestParams::new("musehub_agent_run_respond").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("agent_run_id".into(), json!(Uuid::new_v4().to_string())),
                ("message".into(), json!("nope")),
                ("idempotency_key".into(), json!("ag-resp-missing")),
            ]),
        ))
        .await
        .expect("missing run");
    assert!(wrong_state.is_error.unwrap_or(true));
    let ok = client
        .call_tool(CallToolRequestParams::new("musehub_agent_run_respond").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("agent_run_id".into(), json!(run_id.to_string())),
                ("message".into(), json!("continue please")),
                ("idempotency_key".into(), json!("ag-resp-v1")),
            ]),
        ))
        .await
        .expect("respond");
    assert!(!ok.is_error.unwrap_or(true));
    let msgs: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM agent_run_messages WHERE agent_run_id = $1 AND kind = 'user_input'",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(msgs.0, 1);
    let state: (String,) = sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "queued");
    let conflict = client
        .call_tool(CallToolRequestParams::new("musehub_agent_run_respond").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("agent_run_id".into(), json!(run_id.to_string())),
                ("message".into(), json!("different body")),
                ("idempotency_key".into(), json!("ag-resp-v1")),
            ]),
        ))
        .await;
    assert!(conflict.is_err());
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_pipeline_run_idempotency_replay_and_conflict() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = admin_pat(&app, &cookie, "pipe-idem").await;
    let slug = format!("pipe-idem-{}", Uuid::new_v4().simple());
    let reviewer = format!("rev-pipe-{}", &slug[..8]);
    let reviewer_pass = "Thin-Pipe-Reviewer-12!";
    let (org_id, repo_id, _, head_oid, base_oid) = setup_repo_pr(
        &app,
        &pool,
        git_root.path(),
        &cookie,
        &token,
        &slug,
        &reviewer,
        reviewer_pass,
    )
    .await;
    let pr_id: (Uuid,) = sqlx::query_as("SELECT id FROM pull_requests WHERE repo_id = $1")
        .bind(repo_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "UPDATE pull_requests SET state = 'merged', merged_oid = $2, merged_by = $3, merged_at = CURRENT_TIMESTAMP
         WHERE id = $1",
    )
    .bind(pr_id.0)
    .bind(&head_oid)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(repo_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let bare = bare_repo_path(git_root.path(), &storage_key.0);
    assert!(Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "update-ref",
            "refs/heads/main",
            &head_oid,
        ])
        .status()
        .unwrap()
        .success());
    let client = mcp_client(&app, &token).await;
    let args = serde_json::Map::from_iter([
        ("org_id".into(), json!(org_id.to_string())),
        ("repository_id".into(), json!(repo_id.to_string())),
        ("workflow_path".into(), json!("product-ci")),
        ("target_oid".into(), json!(head_oid)),
        ("idempotency_key".into(), json!("pipe-run-idem-k1")),
    ]);
    let first = client
        .call_tool(CallToolRequestParams::new("musehub_pipeline_run").with_arguments(args.clone()))
        .await
        .expect("first pipeline_run");
    assert!(!first.is_error.unwrap_or(true));
    let run_a = tool_data(&first)["id"].as_str().unwrap().to_string();
    let second = client
        .call_tool(CallToolRequestParams::new("musehub_pipeline_run").with_arguments(args))
        .await
        .expect("replay");
    assert!(!second.is_error.unwrap_or(true));
    let run_b = tool_data(&second)["id"].as_str().unwrap().to_string();
    assert_eq!(run_a, run_b);
    let conflict = client
        .call_tool(CallToolRequestParams::new("musehub_pipeline_run").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.to_string())),
                ("repository_id".into(), json!(repo_id.to_string())),
                ("workflow_path".into(), json!("product-ci")),
                ("target_oid".into(), json!(base_oid)),
                ("idempotency_key".into(), json!("pipe-run-idem-k1")),
            ]),
        ))
        .await;
    assert!(conflict.is_err());
    client.cancel().await.ok();
}
