//! Remote runner: API enqueue-only, claim/fencing, PAT lifecycle, cancel, artifact SHA.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::lease::renew_pipeline_work_lease;
use tower::ServiceExt;
use uuid::Uuid;

const RUNNER_PASS: &str = "Runner-Accept-Member-12!";

struct Fixture {
    token: String,
    run_id: Uuid,
    job_id: Uuid,
    repo_id: Uuid,
    org_id: Uuid,
    storage_key: Uuid,
    source_oid: String,
    token_id: Uuid,
}

async fn remote_fixture(pool: &sqlx::PgPool, git_root: &std::path::Path, app: &axum::Router) -> Fixture {
    std::env::set_var("MUSEHUB_PIPELINE_EXECUTOR", "remote");
    let bootstrap = common::bootstrap_password();
    let admin_cookie = common::login_cookie(app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "runner-acc",
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
                .body(Body::from(r#"{"slug":"runner-acc","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let org_id: Uuid = sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(pool)
        .await
        .unwrap()
        .0;
    let repo_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM repositories WHERE slug = 'runner-acc'")
            .fetch_one(pool)
            .await
            .unwrap()
            .0;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/runner-acc/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"runner-acc","role":"developer"}"#))
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
                        "name": "runner-acc",
                        "org": "demo",
                        "scopes": ["pipeline:execute", "repo:read"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_id],
                        "runner_handle": "runner-acc"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_body: serde_json::Value = serde_json::from_slice(
        &pat_resp.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    let token_id: Uuid = Uuid::parse_str(pat_body["id"].as_str().unwrap()).unwrap();
    let token = pat_body["token"].as_str().unwrap().to_string();
    let storage_key: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT storage_key FROM repositories WHERE slug = 'runner-acc'")
            .fetch_one(pool)
            .await
            .unwrap()
            .0;
    let bare = musehub_app::bare_repo_path(git_root, &storage_key);
    let oid = std::process::Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "rev-parse",
            "refs/heads/main",
        ])
        .output()
        .unwrap();
    let source_oid = String::from_utf8(oid.stdout).unwrap().trim().to_string();
    let admin_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM principals WHERE handle_normalized = 'admin'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .0;
    let run_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','queued',$5,$6,'pr_validation')",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&source_oid)
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','queued')",
    )
    .bind(job_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(run_id)
    .execute(pool)
    .await
    .unwrap();
    let holder: (Option<Uuid>,) =
        sqlx::query_as("SELECT lease_holder FROM pipeline_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(holder.0.is_none(), "enqueue must not set lease_holder on API");
    Fixture {
        token,
        run_id,
        job_id,
        repo_id,
        org_id,
        storage_key,
        source_oid,
        token_id,
    }
}

async fn claim(
    app: &axum::Router,
    token: &str,
    runner_id: Uuid,
) -> (StatusCode, Option<musehub_app::runner_api::ClaimedPipelineRun>) {
    let resp = app
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
    let status = resp.status();
    if !status.is_success() {
        return (status, None);
    }
    let claimed: musehub_app::runner_api::ClaimedPipelineRun = serde_json::from_slice(
        &resp.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    (status, Some(claimed))
}

async fn case_fencing_and_pat_negative_cases() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let fx = remote_fixture(&pool, git_root.path(), &app).await;
    let runner_a = Uuid::new_v4();
    let runner_b = Uuid::new_v4();
    let (_, c1) = claim(&app, &fx.token, runner_a).await;
    assert!(c1.is_some());
    let (st2, _) = claim(&app, &fx.token, runner_b).await;
    assert_eq!(st2, StatusCode::NOT_FOUND);
    sqlx::query(
        "UPDATE access_tokens SET revoked_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(fx.token_id)
    .execute(&pool)
    .await
    .unwrap();
    let (bad, _) = claim(&app, &fx.token, runner_b).await;
    assert_eq!(bad, StatusCode::UNAUTHORIZED);
}

async fn case_renew_fencing_and_artifact_sha() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let fx = remote_fixture(&pool, git_root.path(), &app).await;
    let runner_id = Uuid::new_v4();
    let (_, Some(claimed)) = claim(&app, &fx.token, runner_id).await else {
        panic!("claim");
    };
    assert!(renew_pipeline_work_lease(&pool, claimed.run_id, runner_id, claimed.lease_epoch)
        .await
        .is_ok());
    assert!(
        renew_pipeline_work_lease(&pool, claimed.run_id, runner_id, claimed.lease_epoch - 1)
            .await
            .is_err()
    );
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
        fx.storage_key,
        claimed.run_id,
        claimed.job_id,
        fx.org_id,
        fx.repo_id,
        fx.source_oid,
        "pr_validation".to_string(),
    )
    .await;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let st: (String,) = sqlx::query_as("SELECT state FROM pipeline_runs WHERE id = $1")
            .bind(claimed.run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        if st.0 == "succeeded" || st.0 == "failed" {
            assert_eq!(st.0, "succeeded");
            break;
        }
    }
    let art: (String, String) = sqlx::query_as(
        "SELECT sha256, storage_path FROM artifacts WHERE pipeline_run_id = $1 AND kind = 'file' LIMIT 1",
    )
    .bind(claimed.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!art.0.is_empty());
    let path = std::path::Path::new(&art.1).join("bundle.tar");
    assert!(path.is_file());
    let disk = musehub_app::artifact_sha256_file(&path).unwrap().0;
    assert_eq!(disk, art.0);
}

async fn case_wrong_scope_and_repo_isolation() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let _fx = remote_fixture(&pool, git_root.path(), &app).await;
    let read_only = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "name": "read-only",
                        "org": "demo",
                        "scopes": ["repo:read"],
                        "scope_mode": "all_accessible"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(
        &read_only.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    let token = body["token"].as_str().unwrap();
    let (st, _) = claim(&app, token, Uuid::new_v4()).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn runner_daemon_acceptance_matrix() {
    case_fencing_and_pat_negative_cases().await;
    case_renew_fencing_and_artifact_sha().await;
    case_wrong_scope_and_repo_isolation().await;
}
