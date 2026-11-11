//! PR CI failure view includes honest pipeline job log location (`docs/01-product-requirements.md` §2 S3).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const LOG_LINE: &str = "E2E build failed: compile error at main.rs:12";

#[tokio::test]
async fn pr_ci_failure_surfaces_stored_job_log_location_and_http_logs() {
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
                .body(Body::from(r#"{"slug":"pr-ci-log","description":""}"#))
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
        "SELECT id FROM repositories WHERE slug_normalized = 'pr-ci-log'",
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

    let issue_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, title, state, author_id)
         VALUES ($1,$2,$3,1,'log slice','open',$4)",
    )
    .bind(issue_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(admin_id)
    .execute(&pool)
    .await
    .unwrap();

    let storage_key: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT storage_key FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let bare = git_root.path().join(format!("{storage_key}.git"));
    let head_oid = std::process::Command::new("git")
        .current_dir(&bare)
        .args(["rev-parse", "main"])
        .output()
        .expect("rev-parse")
        .stdout;
    let head_oid = String::from_utf8(head_oid)
        .expect("utf8")
        .trim()
        .to_string();
    let pr_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pull_requests (id, org_id, repo_id, number, author_id, title, source_branch,
         target_branch, state, head_oid, issue_id)
         VALUES ($1,$2,$3,1,$4,'pr-log','feature/log','main','open',$5,$6)",
    )
    .bind(pr_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(admin_id)
    .bind(&head_oid)
    .bind(issue_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO pr_check_runs (id, org_id, repo_id, pr_id, check_name, state, head_oid)
         VALUES ($1,$2,$3,$4,'product-ci','failure',$5)",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(pr_id)
    .bind(&head_oid)
    .execute(&pool)
    .await
    .unwrap();

    let run_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, pr_id, run_kind, error_code)
         VALUES ($1,$2,$3,$4,'product-ci','failed',$5,$6,$7,'pr_validation','build_failed')",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&head_oid)
    .bind(admin_id)
    .bind(attempt_id)
    .bind(pr_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','failed')",
    )
    .bind(job_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_job_logs (id, org_id, repo_id, job_id, attempt_id, chunk_index, text, complete)
         VALUES ($1,$2,$3,$4,$5,0,$6,true)",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(job_id)
    .bind(attempt_id)
    .bind(LOG_LINE)
    .execute(&pool)
    .await
    .unwrap();

    std::process::Command::new("git")
        .current_dir(&bare)
        .args(["update-ref", "refs/heads/feature/log", &head_oid])
        .status()
        .expect("update-ref");

    let ctx_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/repos/pr-ci-log/pull-requests/1/ci-failure")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ctx_resp.status(), StatusCode::OK);
    let ctx: serde_json::Value =
        serde_json::from_slice(&ctx_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        ctx["pipeline_run_id"].as_str(),
        Some(run_id.to_string().as_str())
    );
    assert_eq!(
        ctx["pipeline_run_attempt_id"].as_str(),
        Some(attempt_id.to_string().as_str())
    );
    let jobs = ctx["failed_jobs"].as_array().expect("failed_jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["job_key"].as_str(), Some("build"));
    assert_eq!(jobs[0]["log_has_chunks"].as_bool(), Some(true));
    assert!(jobs[0]["log_excerpt"].as_str().unwrap().contains(LOG_LINE));
    let log_path = jobs[0]["log_api_path"].as_str().unwrap();
    assert!(log_path.contains(&job_id.to_string()));
    assert!(log_path.contains(&attempt_id.to_string()));

    let checks = ctx["checks_failed"].as_array().unwrap();
    assert!(!checks.is_empty());
    assert!(checks[0]["log_location_note"]
        .as_str()
        .unwrap()
        .contains("state only"));

    let logs_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/pr-ci-log/{log_path}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logs_resp.status(), StatusCode::OK);
    let logs: serde_json::Value =
        serde_json::from_slice(&logs_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(logs["text"].as_str().unwrap().contains(LOG_LINE));
}
