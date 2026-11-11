//! Session HTTP: list pipeline jobs and retry failed build job (attempt_id idempotency).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn list_jobs_and_retry_failed_build_idempotent() {
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
                .body(Body::from(r#"{"slug":"pipe-retry-http","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let org_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let repo_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM repositories WHERE slug_normalized = 'pipe-retry-http'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let admin_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let run_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let deploy_job = Uuid::new_v4();
    let oid = "c".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind, error_code)
         VALUES ($1,$2,$3,$4,'product-ci','failed',$5,$6,'release_build','build_failed')",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&oid)
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    for (jid, key, st) in [
        (job_id, "build", "failed"),
        (deploy_job, "deploy", "skipped"),
    ] {
        sqlx::query(
            "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(jid)
        .bind(org_id)
        .bind(repo_id)
        .bind(run_id)
        .bind(key)
        .bind(st)
        .execute(&pool)
        .await
        .unwrap();
    }

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-retry-http/pipeline-runs/{run_id}/jobs"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_bytes = list.into_body().collect().await.unwrap().to_bytes();
    let jobs: serde_json::Value = serde_json::from_slice(&list_bytes).unwrap();
    let arr = jobs.as_array().expect("job array");
    assert_eq!(arr.len(), 2);
    let build = arr
        .iter()
        .find(|j| j["job_key"] == "build")
        .expect("build job");
    assert_eq!(build["retry_allowed"], true);
    let deploy = arr
        .iter()
        .find(|j| j["job_key"] == "deploy")
        .expect("deploy job");
    assert_eq!(deploy["retry_allowed"], false);

    let attempt = Uuid::new_v4();
    let retry_body = serde_json::json!({ "attempt_id": attempt.to_string() }).to_string();
    let retry = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-retry-http/pipeline-jobs/{job_id}/retry"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(retry_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    let job_state: (String,) = sqlx::query_as("SELECT state FROM pipeline_jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(job_state.0, "queued");

    let retry_dup = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-retry-http/pipeline-jobs/{job_id}/retry"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(retry_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry_dup.status(), StatusCode::OK);

    let deploy_retry = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-retry-http/pipeline-jobs/{deploy_job}/retry"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    serde_json::json!({ "attempt_id": Uuid::new_v4().to_string() }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deploy_retry.status(), StatusCode::BAD_REQUEST);
}
