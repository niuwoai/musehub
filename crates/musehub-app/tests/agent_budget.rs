mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use musehub_app::bare_repo_path;
use tower::ServiceExt;

#[tokio::test]
async fn budget_limit_fails_without_commit_on_branch() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"budget","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/budget/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"title":"b"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let n = issue["number"].as_i64().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/budget/issues/{n}/plans"))
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
                .uri(format!("/api/v1/orgs/demo/repos/budget/issues/{n}/plans"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let agent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/budget/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    serde_json::json!({
                        "issue_id": issue["id"],
                        "message": "__budget_exceed__",
                        "budget_tokens": 100
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let agent_json: serde_json::Value =
        serde_json::from_slice(&agent.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let run_id = agent_json["id"].as_str().unwrap();
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/agent-runs/{run_id}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let run: serde_json::Value =
            serde_json::from_slice(&get.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if run["state"] == "failed" || run["state"] == "succeeded" {
            assert_eq!(run["state"], "failed");
            break;
        }
    }
    let branch = format!("agent/{run_id}");
    let storage_key: uuid::Uuid = sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'budget'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    assert_eq!(common::branch_commit_count(&bare, &branch), 0);
}

#[tokio::test]
async fn unknown_budget_with_limit_fails_instead_of_unbounded_success() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"unk","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/unk/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"title":"u"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let n = issue["number"].as_i64().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/unk/issues/{n}/plans"))
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
                .uri(format!("/api/v1/orgs/demo/repos/unk/issues/{n}/plans"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let agent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/unk/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    serde_json::json!({
                        "issue_id": issue["id"],
                        "message": "ok",
                        "budget_tokens": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let run_id = serde_json::from_slice::<serde_json::Value>(
        &agent.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/agent-runs/{run_id}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let run: serde_json::Value =
            serde_json::from_slice(&get.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if run["state"] == "failed" || run["state"] == "succeeded" {
            assert_eq!(run["state"], "failed");
            assert_eq!(run["error_code"].as_str(), Some("agent_budget_unknown"));
            return;
        }
    }
    panic!("agent did not finish");
}
