//! Accepted implementation plan immutability and stale revision rejection.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn accepted_plan_immutable_and_stale_revision_blocks_agent() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"handle":"admin","password":bootstrap}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("login");
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let repo = format!("plan-{}", Uuid::new_v4().simple());
    let repo_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(serde_json::json!({"slug": repo}).to_string()))
                .unwrap(),
        )
        .await
        .expect("repo");
    let repo_status = repo_resp.status();
    let repo_bytes = repo_resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        repo_status,
        StatusCode::CREATED,
        "repo response: {}",
        String::from_utf8_lossy(&repo_bytes)
    );

    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"title":"plan issue"}"#))
                .unwrap(),
        )
        .await
        .expect("issue");
    let issue_status = issue_resp.status();
    let issue_bytes = issue_resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        issue_status,
        StatusCode::CREATED,
        "issue response: {}",
        String::from_utf8_lossy(&issue_bytes)
    );
    let issue_json: serde_json::Value = serde_json::from_slice(&issue_bytes).unwrap();
    let issue_id = Uuid::parse_str(issue_json["id"].as_str().unwrap()).unwrap();
    let issue_number = issue_json["number"].as_i64().unwrap();

    let criteria = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues/{issue_number}/criteria"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    serde_json::json!({
                        "expected_version": 1,
                        "criteria": [{
                            "description": "Regression test passes",
                            "verification_method": "automated",
                            "required": true
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("set issue criteria");
    assert_eq!(criteria.status(), StatusCode::OK);

    let plan = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues/{issue_number}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"body":"v1"}"#))
                .unwrap(),
        )
        .await
        .expect("plan");
    assert_eq!(plan.status(), StatusCode::CREATED);

    let accept = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues/{issue_number}/plans"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("accept");
    assert_eq!(accept.status(), StatusCode::NO_CONTENT);

    let plan_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM implementation_plans WHERE state = 'accepted' LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;

    let mutate = sqlx::query("UPDATE implementation_plans SET body = $2 WHERE id = $1")
        .bind(plan_id)
        .bind("tamper")
        .execute(&pool)
        .await;
    assert!(mutate.is_err());

    let start = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/agent-runs"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    serde_json::json!({"issue_id": issue_id, "message": "implement snapshot"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("start agent run");
    assert_eq!(start.status(), StatusCode::OK);
    let run: serde_json::Value =
        serde_json::from_slice(&start.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let run_id = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();

    let task_message: (String,) =
        sqlx::query_as("SELECT task_message FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let snapshot_text = task_message
        .0
        .split_once("Frozen issue context (JSON):\n")
        .unwrap()
        .1
        .split_once("\n\nAdditional user request:")
        .unwrap()
        .0;
    let snapshot: serde_json::Value = serde_json::from_str(snapshot_text).unwrap();
    assert_eq!(snapshot["issue_revision"], 2);
    assert_eq!(snapshot["issue"]["title"], "plan issue");
    assert_eq!(snapshot["accepted_plan"]["body"], "v1");
    assert_eq!(
        snapshot["acceptance_criteria"][0]["description"],
        "Regression test passes"
    );

    let (org_id, repo_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT org_id, repo_id FROM issues WHERE id = $1")
            .bind(issue_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let rev2 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issue_revisions (id, org_id, repo_id, issue_id, revision, title, body, author_id)
         SELECT $1, org_id, repo_id, id, 3, title, 'rev3', author_id FROM issues WHERE id = $2",
    )
    .bind(rev2)
    .bind(issue_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE issues SET current_revision_id = $2 WHERE id = $1")
        .bind(issue_id)
        .bind(rev2)
        .execute(&pool)
        .await
        .unwrap();
    let _ = (org_id, repo_id);

    let agent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/agent-runs"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    serde_json::json!({"issue_id": issue_id, "message": "stale"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("agent");
    assert_eq!(agent.status(), StatusCode::BAD_REQUEST);
}
