//! REST: issue criteria, environments list, agent respond (AGT-02 / ISS-02 / DEP-02).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn issue_criteria_environments_and_agent_respond_rest() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;

    let repo = format!("iss-rest-{}", Uuid::new_v4().simple());
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "slug": repo }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let envs = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/environments"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(envs.status(), StatusCode::OK);
    let env_json = body_json(envs).await;
    let env_list = env_json.as_array().expect("environments array");
    assert!(env_list.iter().any(|e| e["slug"] == "test"));
    assert!(env_list.iter().any(|e| e["slug"] == "production"));
    assert!(env_list
        .iter()
        .any(|e| e["requires_approval"].as_bool() == Some(true)));

    let issue = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "title": "criteria slice" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(issue.status(), StatusCode::CREATED);

    let detail = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail_json = body_json(detail).await;
    assert_eq!(detail_json["revision"], 1);
    assert!(detail_json["criteria"].as_array().unwrap().is_empty());

    let put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1/criteria"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "expected_version": 1,
                        "criteria": [{
                            "description": "User can sign in",
                            "verification_method": "manual",
                            "required": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::OK);
    let after = body_json(put).await;
    assert_eq!(after["revision"], 2);
    assert_eq!(after["criteria"].as_array().unwrap().len(), 1);

    let trace = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues/1/delivery-trace"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(trace.status(), StatusCode::OK);
    let trace_json = body_json(trace).await;
    assert!(trace_json["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g == "no_pull_requests"));

    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = $1")
        .bind(&repo)
        .fetch_one(&pool)
        .await
        .unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let issue_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM issues WHERE repo_id = $1 AND number = 1")
            .bind(repo_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
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
    .bind(issue_id.0)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();

    let respond = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/agent-runs/{run_id}/respond"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "message": "proceed" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(respond.status(), StatusCode::OK);
    let respond_json = body_json(respond).await;
    assert_eq!(respond_json["state"], "queued");
    assert_eq!(respond_json["message_sequence"], 1);
}
