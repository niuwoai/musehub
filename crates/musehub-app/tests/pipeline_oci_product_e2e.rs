//! Product path: pipeline OCI → publish bind → compose-runner → trace (Docker + local registry).

mod common;

use std::process::Command;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const CI_PASS: &str = "Ci-Delivery-Reporter-12!";
const REVIEWER_PASS: &str = "Delivery-Reviewer-12!";

fn docker_ok() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn api_pipeline_oci_compose_runner_deploy_trace() {
    if !docker_ok() {
        eprintln!("skip: docker unavailable");
        return;
    }
    if std::env::var("MUSEHUB_OCI_REGISTRY").is_err() {
        eprintln!("skip: MUSEHUB_OCI_REGISTRY not set");
        return;
    }
    std::env::set_var("MUSEHUB_REQUIRE_PIPELINE_OCI", "1");
    std::env::set_var("MUSEHUB_COMPOSE_ADAPTER_MODE", "runner");
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    for (handle, pass, role) in [
        ("oci-product-rev", REVIEWER_PASS, "member"),
        ("ci-oci-product", CI_PASS, "admin"),
    ] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/members")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "handle": handle,
                            "password": pass,
                            "role": role
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let ci_cookie = common::login_cookie(&app, "ci-oci-product", CI_PASS).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"oci-product","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/oci-product/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    r#"{"handle":"oci-product-rev","role":"maintainer"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/oci-product/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(r#"{"title":"d"}"#))
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
    let issue_uuid = Uuid::parse_str(issue["id"].as_str().unwrap()).unwrap();
    let n = issue["number"].as_i64().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/oci-product/issues/{n}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
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
                    "/api/v1/orgs/demo/repos/oci-product/issues/{n}/plans"
                ))
                .header(header::COOKIE, &admin_cookie)
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
                    .uri("/api/v1/orgs/demo/repos/oci-product/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "issue_id": issue_uuid,
                            "message": "build"
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
    let mut agent_state = "queued".to_string();
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/v1/agent-runs/{run_id}"))
                        .header(header::COOKIE, &admin_cookie)
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
        agent_state = st["state"].as_str().unwrap_or("").to_string();
        if agent_state == "succeeded" {
            break;
        }
        assert_ne!(
            agent_state, "failed",
            "agent failed: {:?}",
            st["error_code"]
        );
    }
    assert_eq!(agent_state, "succeeded", "agent did not finish");
    let agent_run_uuid = Uuid::parse_str(run_id).unwrap();
    let branch = format!("agent/{run_id}");

    let pr: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/oci-product/pull-requests")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "title": "oci-product",
                            "source_branch": branch,
                            "issue_id": issue_uuid,
                            "agent_run_id": agent_run_uuid
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
    assert!(
        pr.get("number").is_some(),
        "pull-request create failed: {pr}"
    );
    let pr_number = pr["number"].as_i64().unwrap();
    let pr_id = pr["id"].as_str().unwrap();
    let rev_cookie = common::login_cookie(&app, "oci-product-rev", REVIEWER_PASS).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/oci-product/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &rev_cookie)
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/oci-product/pull-requests/{pr_number}/checks/product-ci"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let merged: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/orgs/demo/repos/oci-product/pull-requests/{pr_number}/merge"
                    ))
                    .header(header::COOKIE, &rev_cookie)
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
    let merge_commit = merged["head_oid"].as_str().unwrap();

    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"name":"mcp-oci-pipeline","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let mcp_pat_json: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let mcp_token = mcp_pat_json["token"].as_str().unwrap();
    let repo_uuid: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug = 'oci-product'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let org_uuid: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    use http::HeaderValue;
    use rmcp::{
        model::CallToolRequestParams,
        transport::{
            streamable_http_client::StreamableHttpClientTransportConfig,
            StreamableHttpClientTransport,
        },
        ClientLifecycleMode, ClientServiceExt,
    };
    use std::collections::HashMap;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_addr = listener.local_addr().unwrap();
    let router = app.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    let mut headers = HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {mcp_token}")).unwrap(),
    );
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{mcp_addr}/mcp"))
            .custom_headers(headers),
    );
    let mcp_client = rmcp::model::ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                legacy_version: Some(rmcp::model::ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("mcp client");
    let pipe_mcp = mcp_client
        .call_tool(
            CallToolRequestParams::new("musehub_pipeline_run").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), serde_json::json!(org_uuid.0.to_string()));
                m.insert(
                    "repository_id".into(),
                    serde_json::json!(repo_uuid.0.to_string()),
                );
                m.insert("workflow_path".into(), serde_json::json!("product-ci"));
                m.insert("target_oid".into(), serde_json::json!(merge_commit));
                m.insert(
                    "idempotency_key".into(),
                    serde_json::json!("oci-mcp-pipeline-v1"),
                );
                m
            }),
        )
        .await
        .expect("mcp pipeline_run");
    assert!(!pipe_mcp.is_error.unwrap_or(true));
    let pipeline_id = pipe_mcp
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .expect("pipeline id")
        .to_string();
    let pipe_replay = mcp_client
        .call_tool(
            CallToolRequestParams::new("musehub_pipeline_run").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), serde_json::json!(org_uuid.0.to_string()));
                m.insert(
                    "repository_id".into(),
                    serde_json::json!(repo_uuid.0.to_string()),
                );
                m.insert("workflow_path".into(), serde_json::json!("product-ci"));
                m.insert("target_oid".into(), serde_json::json!(merge_commit));
                m.insert(
                    "idempotency_key".into(),
                    serde_json::json!("oci-mcp-pipeline-v1"),
                );
                m
            }),
        )
        .await
        .expect("mcp pipeline_run replay");
    assert!(!pipe_replay.is_error.unwrap_or(true));
    let pipeline_replay_id = pipe_replay
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .expect("pipeline replay id");
    assert_eq!(pipeline_replay_id, pipeline_id.as_str());
    for _ in 0..600 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!(
                            "/api/v1/orgs/demo/repos/oci-product/pipeline-runs/{pipeline_id}"
                        ))
                        .header(header::COOKIE, &admin_cookie)
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
            assert_eq!(
                st["state"], "succeeded",
                "pipeline error={:?}",
                st["error_code"]
            );
            break;
        }
    }

    let rel: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/oci-product/releases")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "version": "0.1.0-rc4.1",
                            "tag_name": "v0.1.0-rc4.1",
                            "commit_oid": merge_commit,
                            "pipeline_run_id": pipeline_id,
                            "pr_id": pr_id
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
    let release_id = rel["id"].as_str().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/oci-product/releases/{release_id}/publish"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let env_uuid: (Uuid,) =
        sqlx::query_as("SELECT id FROM environments WHERE repo_id = $1 AND slug = 'test'")
            .bind(repo_uuid.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let dep_mcp = mcp_client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_start").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), serde_json::json!(org_uuid.0.to_string()));
                m.insert(
                    "repository_id".into(),
                    serde_json::json!(repo_uuid.0.to_string()),
                );
                m.insert(
                    "environment_id".into(),
                    serde_json::json!(env_uuid.0.to_string()),
                );
                m.insert("release_id".into(), serde_json::json!(release_id));
                m.insert(
                    "idempotency_key".into(),
                    serde_json::json!("oci-mcp-deploy-v1"),
                );
                m
            }),
        )
        .await
        .expect("mcp deployment_start");
    assert!(!dep_mcp.is_error.unwrap_or(true));
    let dep_data = dep_mcp
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .expect("deploy data");
    assert_eq!(
        dep_data.get("state").and_then(|v| v.as_str()),
        Some("succeeded")
    );
    assert_eq!(
        dep_data.get("adapter_kind").and_then(|v| v.as_str()),
        Some("compose-runner")
    );
    let deployment_id = dep_data
        .get("id")
        .and_then(|v| v.as_str())
        .expect("deployment id");

    let trace: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/v1/orgs/demo/repos/oci-product/trace/deployments/{deployment_id}"
                    ))
                    .header(header::COOKIE, &admin_cookie)
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
    assert_eq!(trace["pipeline_run_id"].as_str().unwrap(), pipeline_id);
    assert_eq!(trace["commit_oid"].as_str().unwrap(), merge_commit);
    assert!(trace["artifacts"].as_array().is_some_and(|a| !a.is_empty()));
    assert_eq!(trace["agent_run_id"].as_str().unwrap(), run_id);
    assert_eq!(trace["adapter_is_mock"].as_bool(), Some(false));
    assert_eq!(trace["deployment_adapter"], "compose-runner");
    assert_eq!(trace["verified_source_oid"].as_str().unwrap(), merge_commit);
    assert!(trace["release_oci_image_ref"]
        .as_str()
        .is_some_and(|r| r.contains("@sha256:")));
    let gaps = trace["gaps"].as_array().unwrap();
    let gap_text: Vec<&str> = gaps.iter().filter_map(|g| g.as_str()).collect();
    assert!(
        !gap_text.contains(&"deploy_no_oci_registry"),
        "product path should not report deploy_no_oci_registry: {gap_text:?}"
    );
    assert!(
        !gap_text.contains(&"mcp_production_not_wired"),
        "product MCP slice removes stale mcp_production_not_wired gap"
    );

    let trace_mcp = mcp_client
        .call_tool(
            CallToolRequestParams::new("musehub_deployment_trace").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), serde_json::json!(org_uuid.0.to_string()));
                m.insert(
                    "repository_id".into(),
                    serde_json::json!(repo_uuid.0.to_string()),
                );
                m.insert("deployment_id".into(), serde_json::json!(deployment_id));
                m
            }),
        )
        .await
        .expect("mcp trace");
    assert!(!trace_mcp.is_error.unwrap_or(true));
    let gaps_mcp = trace_mcp
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("gaps"));
    assert!(gaps_mcp.is_some());
    let prompts = mcp_client.list_prompts(None).await.expect("prompts");
    assert!(!prompts.prompts.is_empty());
    let resources = mcp_client.list_resources(None).await.expect("resources");
    assert!(!resources.resources.is_empty());
    mcp_client.cancel().await.ok();

    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("python/mcp_product_oci_read_probe.py");
    let pydeps =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/mcp-matrix-pydeps");
    let py_ok = std::process::Command::new("python3")
        .args(["-c", "import mcp"])
        .env("PYTHONPATH", &pydeps)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if py_ok && script.is_file() {
        let py_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let py_addr = py_listener.local_addr().unwrap();
        let py_router = app.clone();
        tokio::spawn(async move {
            let _ = axum::serve(py_listener, py_router).await;
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        let url = format!("http://{py_addr}/mcp");
        let mcp_token_own = mcp_token.to_string();
        let deployment_id_own = deployment_id.to_string();
        let org_s = org_uuid.0.to_string();
        let repo_s = repo_uuid.0.to_string();
        let pydeps_read = pydeps.clone();
        let out = tokio::task::spawn_blocking(move || {
            std::process::Command::new("python3")
                .env("PYTHONPATH", &pydeps_read)
                .arg(&script)
                .arg("--url")
                .arg(&url)
                .arg("--bearer")
                .arg(&mcp_token_own)
                .arg("--org-id")
                .arg(org_s)
                .arg("--repository-id")
                .arg(repo_s)
                .arg("--deployment-id")
                .arg(&deployment_id_own)
                .output()
        })
        .await
        .expect("join python oci read probe")
        .expect("python oci read probe");
        assert!(
            out.status.success(),
            "python oci read probe: stderr={} stdout={}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
    } else {
        eprintln!("skip python oci mcp: SDK or script missing");
    }

    let write_script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("python/mcp_product_oci_write_probe.py");
    if py_ok && write_script.is_file() {
        let py_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let py_addr = py_listener.local_addr().unwrap();
        let py_router = app.clone();
        tokio::spawn(async move {
            let _ = axum::serve(py_listener, py_router).await;
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        let url = format!("http://{py_addr}/mcp");
        let api_base = format!("http://{py_addr}");
        let mcp_token_own = mcp_token.to_string();
        let _admin_cookie_own = admin_cookie.clone();
        let release_id_own = release_id.to_string();
        let merge_commit_own = merge_commit.to_string();
        let org_s = org_uuid.0.to_string();
        let repo_s = repo_uuid.0.to_string();
        let env_s = env_uuid.0.to_string();
        let pydeps_owned = pydeps.clone();
        let write_script_owned = write_script.clone();
        let out = tokio::task::spawn_blocking(move || {
            std::process::Command::new("python3")
                .env("PYTHONPATH", &pydeps_owned)
                .arg(&write_script_owned)
                .arg("--url")
                .arg(&url)
                .arg("--bearer")
                .arg(&mcp_token_own)
                .arg("--session-cookie")
                .arg(&admin_cookie)
                .arg("--api-base")
                .arg(&api_base)
                .arg("--org-slug")
                .arg("demo")
                .arg("--repo-slug")
                .arg("oci-product")
                .arg("--org-id")
                .arg(org_s)
                .arg("--repository-id")
                .arg(repo_s)
                .arg("--environment-id")
                .arg(env_s)
                .arg("--release-id")
                .arg(release_id_own)
                .arg("--mode")
                .arg("full")
                .arg("--target-oid")
                .arg(merge_commit_own)
                .output()
        })
        .await
        .expect("join python write probe")
        .expect("python write probe");
        assert!(
            out.status.success(),
            "python oci write probe (full): stderr={} stdout={}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
