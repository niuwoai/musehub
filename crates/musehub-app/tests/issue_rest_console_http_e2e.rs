//! REST: issue criteria, environments list, agent respond (AGT-02 / ISS-02 / DEP-02).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::s1_git_push::push_branch;
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

    let assignees = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issue-assignees"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(assignees.status(), StatusCode::OK);
    let assignee_json = body_json(assignees).await;
    assert!(
        assignee_json
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["handle"] == "admin"),
        "org admin must be assignee-eligible"
    );

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

    let close_missing_reason = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "state": "closed" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_missing_reason.status(), StatusCode::BAD_REQUEST);

    let close_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "state": "closed", "close_reason": "accepted" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_ok.status(), StatusCode::OK);
    let closed_json = body_json(close_ok).await;
    assert_eq!(closed_json["state"], "closed");
    assert_eq!(closed_json["close_reason"], "accepted");

    let patch_reason_only = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "close_reason": "duplicate" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_reason_only.status(), StatusCode::OK);
    let reason_only_json = body_json(patch_reason_only).await;
    assert_eq!(reason_only_json["close_reason"], "duplicate");

    let in_progress = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "state": "in_progress" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(in_progress.status(), StatusCode::OK);
    let in_progress_json = body_json(in_progress).await;
    assert_eq!(in_progress_json["state"], "in_progress");
    assert!(in_progress_json["close_reason"].is_null());

    let list_in_progress = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?state=in_progress"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_in_progress.status(), StatusCode::OK);
    let in_prog_list = body_json(list_in_progress).await;
    assert!(
        in_prog_list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["number"] == 1),
        "in_progress filter must return issue #1"
    );

    let bad_state = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?state=not-a-state"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_state.status(), StatusCode::BAD_REQUEST);

    let bad_assignee = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?assignee=no-such-collaborator"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_assignee.status(), StatusCode::BAD_REQUEST);

    let mark_open = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "state": "open" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mark_open.status(), StatusCode::OK);
    assert_eq!(body_json(mark_open).await["state"], "open");

    let list_open_after_progress = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues?state=open"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_open_after_progress.status(), StatusCode::OK);
    assert!(body_json(list_open_after_progress).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["number"] == 1));

    let tag_issue = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "labels": ["v1-console"],
                        "assignee_handle": "admin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tag_issue.status(), StatusCode::OK);

    let list_label = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?label=v1-console"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_label.status(), StatusCode::OK);
    let list_label_json = body_json(list_label).await;
    let label_items = list_label_json["items"].as_array().unwrap();
    assert_eq!(label_items.len(), 1);
    assert!(label_items[0]["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l == "v1-console"));

    let list_assignee_admin = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?assignee=admin"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_assignee_admin.status(), StatusCode::OK);
    assert!(body_json(list_assignee_admin).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["number"] == 1 && i["assignee_handle"] == "admin"));

    let bad_label = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?label=bad%21"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_label.status(), StatusCode::BAD_REQUEST);

    let list_label_trim = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?label=%20v1-console%20"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_label_trim.status(), StatusCode::OK);
    assert_eq!(
        body_json(list_label_trim).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    for title in ["slice two", "slice three", "slice four"] {
        let created_n = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "title": title }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created_n.status(), StatusCode::CREATED);
        let n = body_json(created_n).await["number"].as_i64().unwrap();
        let tag = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/{n}"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "labels": ["v1-console"] }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tag.status(), StatusCode::OK);
    }

    let page_one = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?label=v1-console&limit=2"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page_one.status(), StatusCode::OK);
    let page_one_json = body_json(page_one).await;
    assert!(page_one_json["has_more"].as_bool() == Some(true));
    let cursor = page_one_json["next_cursor"]
        .as_str()
        .expect("cursor for label page");
    let first_ids: Vec<String> = page_one_json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(first_ids.len(), 2);

    let page_two = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?label=v1-console&limit=2&cursor={cursor}"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page_two.status(), StatusCode::OK);
    let page_two_json = body_json(page_two).await;
    let second_ids: Vec<String> = page_two_json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    for id in &second_ids {
        assert!(!first_ids.contains(id));
    }
    let total = first_ids.len() + second_ids.len();
    assert!(total >= 3);
    for id in first_ids.iter().chain(second_ids.iter()) {
        let listed = page_one_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .chain(page_two_json["items"].as_array().unwrap().iter())
            .find(|i| i["id"].as_str() == Some(id.as_str()))
            .unwrap();
        assert!(listed["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "v1-console"));
    }

    let reason_while_open = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "close_reason": "nope" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reason_while_open.status(), StatusCode::BAD_REQUEST);

    let close_again = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "state": "closed", "close_reason": "accepted" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_again.status(), StatusCode::OK);

    let list_closed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?state=closed"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_closed.status(), StatusCode::OK);
    let list_json = body_json(list_closed).await;
    assert_eq!(list_json["items"].as_array().unwrap().len(), 1);

    let reopen = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues/1"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "state": "open" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reopen.status(), StatusCode::OK);
    assert!(body_json(reopen).await["close_reason"].is_null());

    let repo_b = format!("pr-run-b-{}", Uuid::new_v4().simple());
    let created_b = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "slug": repo_b }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created_b.status(), StatusCode::CREATED);

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

    let storage_b: (Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug_normalized = $1")
            .bind(&repo_b)
            .fetch_one(&pool)
            .await
            .unwrap();
    push_branch(
        &git_root,
        storage_b.0,
        "feat/cross",
        "branch.txt",
        b"diff\n",
    );
    let cross_pr = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo_b}/pull-requests"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "title": "foreign agent run",
                        "source_branch": "feat/cross",
                        "agent_run_id": run_id,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_pr.status(), StatusCode::NOT_FOUND);

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
