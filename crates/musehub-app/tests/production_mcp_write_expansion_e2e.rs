//! MCP write expansion: repository_create, issue_update, pr_create/list, guards.

mod common;

use std::collections::HashMap;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use http_body_util::BodyExt;
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

async fn issue_pat(app: &axum::Router, cookie: &str, body: String) -> String {
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    pat_json["token"].as_str().unwrap().to_string()
}

fn tool_data(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let v = result
        .structured_content
        .as_ref()
        .expect("structured")
        .clone();
    v.get("data").cloned().unwrap_or(v)
}

fn push_feature_branch(git_root: &std::path::Path, storage_key: Uuid, branch: &str) -> String {
    let bare = bare_repo_path(git_root, &storage_key);
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    assert!(Command::new("git")
        .args(["clone", bare.to_str().unwrap(), work.to_str().unwrap(),])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(work)
        .args(["checkout", "-b", branch])
        .status()
        .unwrap()
        .success());
    std::fs::write(work.join("feature.txt"), b"mcp-pr-create\n").unwrap();
    assert!(Command::new("git")
        .current_dir(work)
        .args(["add", "feature.txt"])
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
            "feat"
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

#[tokio::test]
async fn mcp_repository_create_idempotent_and_issue_update_revision() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = issue_pat(
        &app,
        &cookie,
        r#"{"name":"exp-write","org":"demo"}"#.to_string(),
    )
    .await;
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client = mcp_client(&app, &token).await;
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let slug = format!("mcp-exp-{}", Uuid::new_v4().simple());
    let key = "repo-create-idem-v1";
    let args = serde_json::Map::from_iter([
        ("org_id".into(), json!(org_id.0.to_string())),
        ("slug".into(), json!(slug.clone())),
        ("idempotency_key".into(), json!(key)),
        ("description".into(), json!("via mcp")),
    ]);
    let first = client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_create").with_arguments(args.clone()),
        )
        .await
        .expect("create");
    assert!(!first.is_error.unwrap_or(true));
    let repo_id = tool_data(&first)["id"].as_str().unwrap().to_string();
    let second = client
        .call_tool(CallToolRequestParams::new("musehub_repository_create").with_arguments(args))
        .await
        .expect("replay");
    assert!(!second.is_error.unwrap_or(true));
    assert_eq!(tool_data(&second)["id"].as_str().unwrap(), repo_id.as_str());
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM repositories WHERE org_id = $1 AND slug_normalized = $2",
    )
    .bind(org_id.0)
    .bind(slug.to_lowercase())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1);

    let issue_args = serde_json::Map::from_iter([
        ("org_id".into(), json!(org_id.0.to_string())),
        ("repository_id".into(), json!(repo_id)),
        ("idempotency_key".into(), json!("issue-v1")),
        ("title".into(), json!("before")),
        ("body".into(), json!("body1")),
        ("labels".into(), json!(["bug", "mcp"])),
        ("assignee_id".into(), json!(admin_id.0.to_string())),
    ]);
    let issue = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(issue_args.clone()),
        )
        .await
        .expect("issue");
    assert!(!issue.is_error.unwrap_or(true));
    let issue_data = tool_data(&issue);
    assert_eq!(issue_data["assignee_id"], admin_id.0.to_string());
    assert_eq!(issue_data["labels"], json!(["bug", "mcp"]));
    let mut changed_assignee_args = issue_args.clone();
    changed_assignee_args.insert("assignee_id".into(), json!(Uuid::new_v4().to_string()));
    let changed_assignee = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create")
                .with_arguments(changed_assignee_args),
        )
        .await;
    assert!(
        changed_assignee.is_err()
            || changed_assignee
                .as_ref()
                .is_ok_and(|result| result.is_error.unwrap_or(false)),
        "reusing an Issue idempotency key with a different assignee must conflict"
    );
    let invalid_assignee = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("idempotency_key".into(), json!("issue-invalid-assignee")),
                    ("title".into(), json!("invalid assignee")),
                    ("assignee_id".into(), json!(Uuid::new_v4().to_string())),
                ]),
            ),
        )
        .await
        .expect("invalid assignee envelope");
    assert!(invalid_assignee.is_error.unwrap_or(false));
    let issue_id = issue_data["id"].as_str().unwrap().to_string();
    let update = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_update").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("issue_id".into(), json!(issue_id)),
                    ("expected_version".into(), json!(1)),
                    ("title".into(), json!("after")),
                    ("idempotency_key".into(), json!("issue-upd-v1")),
                ]),
            ),
        )
        .await
        .expect("update");
    assert!(!update.is_error.unwrap_or(true));
    assert_eq!(tool_data(&update)["title"], "after");
    assert_eq!(tool_data(&update)["assignee_id"], admin_id.0.to_string());
    assert_eq!(tool_data(&update)["version"], 2);

    let clear_assignee = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_update").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("issue_id".into(), json!(issue_id)),
                    ("expected_version".into(), json!(2)),
                    ("assignee_id".into(), json!(null)),
                    ("idempotency_key".into(), json!("issue-clear-assignee")),
                ]),
            ),
        )
        .await
        .expect("clear assignee");
    assert!(!clear_assignee.is_error.unwrap_or(true));
    assert!(tool_data(&clear_assignee)["assignee_id"].is_null());
    assert_eq!(tool_data(&clear_assignee)["version"], 2);

    let set_assignee = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_update").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("issue_id".into(), json!(issue_id)),
                    ("expected_version".into(), json!(2)),
                    ("assignee_id".into(), json!(admin_id.0.to_string())),
                    ("labels".into(), json!(["bug", "patched"])),
                    ("idempotency_key".into(), json!("issue-set-assignee")),
                ]),
            ),
        )
        .await
        .expect("set assignee");
    assert!(!set_assignee.is_error.unwrap_or(true));
    assert_eq!(
        tool_data(&set_assignee)["assignee_id"],
        admin_id.0.to_string()
    );
    assert_eq!(tool_data(&set_assignee)["version"], 2);
    assert_eq!(
        tool_data(&set_assignee)["labels"],
        json!(["bug", "patched"])
    );
    let resolved = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_update").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("issue_id".into(), json!(issue_id)),
                    ("expected_version".into(), json!(2)),
                    ("state".into(), json!("resolved")),
                    ("resolution_reason".into(), json!("acceptance met")),
                    ("idempotency_key".into(), json!("issue-resolve")),
                ]),
            ),
        )
        .await
        .expect("resolve issue");
    assert!(!resolved.is_error.unwrap_or(true));
    assert_eq!(tool_data(&resolved)["state"], "resolved");
    assert_eq!(tool_data(&resolved)["version"], 2);
    let second_issue = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("idempotency_key".into(), json!("issue-page-2")),
                    ("title".into(), json!("second issue")),
                ]),
            ),
        )
        .await
        .expect("create second issue");
    assert!(!second_issue.is_error.unwrap_or(true));
    let second_issue_id = tool_data(&second_issue)["id"].as_str().unwrap().to_string();
    let filtered = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_list").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("assignee_id".into(), json!(admin_id.0.to_string())),
                    ("label".into(), json!("bug")),
                ]),
            ),
        )
        .await
        .expect("filter issues by assignee and label");
    assert!(!filtered.is_error.unwrap_or(true));
    assert!(tool_data(&filtered)["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == issue_id));
    let get = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_get").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("issue_id".into(), json!(issue_id)),
                ]),
            ),
        )
        .await
        .expect("get Issue assignee");
    assert_eq!(tool_data(&get)["assignee_id"], admin_id.0.to_string());
    let first_page = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_list").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("limit".into(), json!(1)),
                ]),
            ),
        )
        .await
        .expect("first issue page");
    assert!(!first_page.is_error.unwrap_or(true));
    assert_eq!(tool_data(&first_page)["items"][0]["id"], issue_id);
    let page_meta = &first_page.structured_content.as_ref().expect("structured")["page"];
    assert_eq!(page_meta["has_more"], true);
    let cursor = page_meta["next_cursor"]
        .as_str()
        .expect("next issue cursor")
        .to_string();
    let second_page = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_list").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("limit".into(), json!(1)),
                    ("cursor".into(), json!(cursor)),
                ]),
            ),
        )
        .await
        .expect("second issue page");
    assert!(!second_page.is_error.unwrap_or(true));
    assert_eq!(tool_data(&second_page)["items"][0]["id"], second_issue_id);
    let invalid_assignee_filter = client
        .call_tool(
            CallToolRequestParams::new("musehub_issue_list").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("assignee_id".into(), json!(Uuid::new_v4().to_string())),
                ]),
            ),
        )
        .await
        .expect("invalid assignee filter envelope");
    assert!(invalid_assignee_filter.is_error.unwrap_or(false));
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_repository_create_member_forbidden_and_cross_org() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let member_pass = "Mcp-Member-Write-12!";
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(format!(
                    r#"{{"handle":"mcp-member","password":"{member_pass}","role":"member"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let member_cookie = common::login_cookie(&app, "mcp-member", member_pass).await;
    let member_token = issue_pat(
        &app,
        &member_cookie,
        r#"{"name":"member-write","org":"demo"}"#.to_string(),
    )
    .await;
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client = mcp_client(&app, &member_token).await;
    let denied = client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("slug".into(), json!("member-denied")),
                    ("idempotency_key".into(), json!("denied-key")),
                ]),
            ),
        )
        .await
        .expect("call");
    assert!(
        denied.is_error.unwrap_or(false) || tool_data(&denied).is_null(),
        "member must not create repository via MCP"
    );

    let admin_token = issue_pat(
        &app,
        &admin_cookie,
        r#"{"name":"admin-exp","org":"demo"}"#.to_string(),
    )
    .await;
    let admin_client = mcp_client(&app, &admin_token).await;
    let other_org = Uuid::new_v4();
    let cross = admin_client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(other_org.to_string())),
                    ("slug".into(), json!("cross-org")),
                    ("idempotency_key".into(), json!("cross-key")),
                ]),
            ),
        )
        .await
        .expect("cross");
    assert!(
        cross.is_error.unwrap_or(false) || tool_data(&cross).is_null(),
        "cross-org repository_create must fail"
    );
    admin_client.cancel().await.ok();
    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_pr_create_and_list_on_branch() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = issue_pat(
        &app,
        &cookie,
        r#"{"name":"pr-exp","org":"demo"}"#.to_string(),
    )
    .await;
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let client = mcp_client(&app, &token).await;
    let slug = format!("mcp-pr-{}", Uuid::new_v4().simple());
    let create_repo = client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("slug".into(), json!(slug.clone())),
                    ("idempotency_key".into(), json!("pr-repo-key")),
                ]),
            ),
        )
        .await
        .expect("repo");
    assert!(!create_repo.is_error.unwrap_or(true));
    let repo_data = tool_data(&create_repo);
    let repo_id = repo_data["id"].as_str().unwrap();
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(Uuid::parse_str(repo_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    let head = push_feature_branch(git_root.path(), storage_key.0, "feature/mcp");
    let pr = client
        .call_tool(
            CallToolRequestParams::new("musehub_pr_create").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("title".into(), json!("MCP PR")),
                    ("body".into(), json!("")),
                    ("source_branch".into(), json!("feature/mcp")),
                    ("target_branch".into(), json!("main")),
                    ("expected_head_oid".into(), json!(head)),
                    ("issue_revision_ids".into(), json!([])),
                    ("draft".into(), json!(false)),
                    ("idempotency_key".into(), json!("pr-create-key")),
                ]),
            ),
        )
        .await
        .expect("pr create");
    assert!(!pr.is_error.unwrap_or(true), "{pr:?}");
    let listed = client
        .call_tool(
            CallToolRequestParams::new("musehub_pr_list").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id)),
                    ("state".into(), json!("open")),
                ]),
            ),
        )
        .await
        .expect("pr list");
    assert!(!listed.is_error.unwrap_or(true));
    let list_data = tool_data(&listed);
    let items = list_data["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    client.cancel().await.ok();
}
