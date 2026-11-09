//! Remote pipeline executor: claim via PAT + execute on worker instance id.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const RUNNER_PASS: &str = "Remote-Runner-Member-12!";

#[tokio::test]
async fn remote_runner_claims_and_completes_pipeline() {
    std::env::set_var("MUSEHUB_PIPELINE_EXECUTOR", "remote");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "remote-runner",
                        "password": RUNNER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"remote-pipe","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'remote-pipe'")
        .fetch_one(&pool)
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/remote-pipe/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "remote-runner",
                        "role": "developer"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "name": "runner-remote",
                        "org": "demo",
                        "scopes": ["pipeline:execute", "repo:read"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_id.0],
                        "runner_handle": "remote-runner"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_body: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = pat_body["token"].as_str().unwrap();
    let runner_id = Uuid::new_v4();
    let storage_key: (Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug = 'remote-pipe'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let bare = musehub_app::bare_repo_path(git_root.path(), &storage_key.0);
    let oid = std::process::Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "rev-parse",
            "refs/heads/main",
        ])
        .output()
        .unwrap();
    assert!(oid.status.success());
    let source_oid = String::from_utf8(oid.stdout).unwrap().trim().to_string();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let run_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let attempt = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','queued',$5,$6,'pr_validation')",
    )
    .bind(run_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&source_oid)
    .bind(admin_id.0)
    .bind(attempt)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','queued')",
    )
    .bind(job_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap();
    let claim = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/runner/pipeline-runs/claim")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-musehub-runner-id", runner_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(claim.status(), StatusCode::OK);
    let claimed: musehub_app::runner_api::ClaimedPipelineRun =
        serde_json::from_slice(&claim.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(claimed.run_id, run_id);
    assert_eq!(claimed.job_id, job_id);
    let supervisor = musehub_app::pipeline_supervisor::PipelineSupervisor::new();
    musehub_app::execute_pipeline_job(
        pool.clone(),
        git_root.path().to_path_buf(),
        git_root.path().join("artifacts"),
        std::env::var("CARGO_BIN_EXE_musehub-pipeline-reference")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-pipeline-reference")
            }),
        supervisor,
        runner_id,
        storage_key.0,
        claimed.run_id,
        claimed.job_id,
        org_id.0,
        repo_id.0,
        claimed.source_oid,
        claimed.run_kind,
    )
    .await;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let st: (String,) = sqlx::query_as("SELECT state FROM pipeline_runs WHERE id = $1")
            .bind(claimed.run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        if st.0 == "succeeded" || st.0 == "failed" {
            assert_eq!(st.0, "succeeded");
            return;
        }
    }
    panic!("pipeline did not finish");
    #[allow(unreachable_code)]
    {
        std::env::remove_var("MUSEHUB_PIPELINE_EXECUTOR");
    }
}
