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

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"plan","description":""}"#))
                .unwrap(),
        )
        .await
        .expect("repo");

    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/plan/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"title":"plan issue"}"#))
                .unwrap(),
        )
        .await
        .expect("issue");
    let issue_json: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let issue_id = Uuid::parse_str(issue_json["id"].as_str().unwrap()).unwrap();
    let issue_number = issue_json["number"].as_i64().unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/plan/issues/{issue_number}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"body":"v1"}"#))
                .unwrap(),
        )
        .await
        .expect("plan");

    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/plan/issues/{issue_number}/plans"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("accept");

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

    let (org_id, repo_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT org_id, repo_id FROM issues WHERE id = $1")
            .bind(issue_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let rev2 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issue_revisions (id, org_id, repo_id, issue_id, revision, title, body, author_id)
         SELECT $1, org_id, repo_id, id, 2, title, 'rev2', author_id FROM issues WHERE id = $2",
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
                .uri("/api/v1/orgs/demo/repos/plan/agent-runs")
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
