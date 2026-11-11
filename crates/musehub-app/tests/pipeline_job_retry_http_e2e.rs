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

    let mismatch_run = Uuid::new_v4();
    let mismatch_job = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind, error_code, cancel_requested)
         VALUES ($1,$2,$3,$4,'product-ci','cancel_requested',$5,$6,'release_build','build_failed',TRUE)",
    )
    .bind(mismatch_run)
    .bind(org_id)
    .bind(repo_id)
    .bind(&oid)
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','failed')",
    )
    .bind(mismatch_job)
    .bind(org_id)
    .bind(repo_id)
    .bind(mismatch_run)
    .execute(&pool)
    .await
    .unwrap();
    let mismatch_retry = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pipe-retry-http/pipeline-jobs/{mismatch_job}/retry"
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
    assert_eq!(mismatch_retry.status(), StatusCode::CONFLICT);
    let mismatch_job_state: (String,) =
        sqlx::query_as("SELECT state FROM pipeline_jobs WHERE id = $1")
            .bind(mismatch_job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mismatch_job_state.0, "failed");
    let mismatch_run_row: (String, Option<String>) =
        sqlx::query_as("SELECT state, error_code FROM pipeline_runs WHERE id = $1")
            .bind(mismatch_run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mismatch_run_row.0, "cancel_requested");
    assert_eq!(mismatch_run_row.1.as_deref(), Some("build_failed"));

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"pipe-retry-race","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let race_repo_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM repositories WHERE slug_normalized = 'pipe-retry-race'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let race_run_id = Uuid::new_v4();
    let race_job_id = Uuid::new_v4();
    let race_oid = "d".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind, error_code)
         VALUES ($1,$2,$3,$4,'product-ci','failed',$5,$6,'release_build','build_failed')",
    )
    .bind(race_run_id)
    .bind(org_id)
    .bind(race_repo_id)
    .bind(&race_oid)
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','failed')",
    )
    .bind(race_job_id)
    .bind(org_id)
    .bind(race_repo_id)
    .bind(race_run_id)
    .execute(&pool)
    .await
    .unwrap();

    let attempt_a = Uuid::new_v4();
    let attempt_b = Uuid::new_v4();
    let uri = format!("/api/v1/orgs/demo/repos/pipe-retry-race/pipeline-jobs/{race_job_id}/retry");
    let body_a = serde_json::json!({ "attempt_id": attempt_a.to_string() }).to_string();
    let body_b = serde_json::json!({ "attempt_id": attempt_b.to_string() }).to_string();

    let (resp_a, resp_b) = tokio::join!(
        app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(body_a))
                .unwrap(),
        ),
        app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(body_b))
                .unwrap(),
        ),
    );
    let resp_a = resp_a.unwrap();
    let resp_b = resp_b.unwrap();
    let ok_count = [resp_a.status(), resp_b.status()]
        .iter()
        .filter(|s| **s == StatusCode::OK)
        .count();
    let bad_count = [resp_a.status(), resp_b.status()]
        .iter()
        .filter(|s| **s == StatusCode::BAD_REQUEST)
        .count();
    assert_eq!(ok_count, 1, "exactly one retry succeeds");
    assert_eq!(bad_count, 1, "loser rejected without partial retry");

    let job_state: (String,) = sqlx::query_as("SELECT state FROM pipeline_jobs WHERE id = $1")
        .bind(race_job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(matches!(job_state.0.as_str(), "queued" | "running"));
    let run_row: (String, Option<String>) =
        sqlx::query_as("SELECT state, error_code FROM pipeline_runs WHERE id = $1")
            .bind(race_run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(matches!(run_row.0.as_str(), "queued" | "running"));
    assert!(run_row.1.is_none());
}
