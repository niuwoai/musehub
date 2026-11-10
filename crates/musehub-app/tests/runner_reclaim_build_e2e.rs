//! After lease TTL / worker loss, a new worker completes the build once (epoch fencing).

mod common;

use std::process::Stdio;
use std::time::Duration;

use musehub_app::bare_repo_path;
use musehub_app::execute_pipeline_job;
use musehub_app::lease::{self, try_reclaim_expired_pipeline};
use musehub_app::pipeline_supervisor::PipelineSupervisor;
use uuid::Uuid;

#[tokio::test]
async fn reclaim_expired_worker_completes_once() {
    std::env::set_var("MUSEHUB_LEASE_SECS", "8");
    std::env::set_var("MUSEHUB_PIPELINE_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_PIPELINE_HOLD_SECS", "300");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"reclaim-pipe","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let org_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let repo_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM repositories WHERE slug = 'reclaim-pipe'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let storage_key: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT storage_key FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    let source_oid = std::process::Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "rev-parse",
            "refs/heads/main",
        ])
        .output()
        .map(|o| String::from_utf8(o.stdout).unwrap().trim().to_string())
        .unwrap();
    let admin_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
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
    .execute(&pool)
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
    .execute(&pool)
    .await
    .unwrap();

    let worker_a = Uuid::new_v4();
    let lease_a = lease::try_begin_pipeline_work(&pool, run_id, worker_a)
        .await
        .expect("worker A claims");
    let renew = lease::spawn_pipeline_lease_renewal(pool.clone(), run_id, worker_a, lease_a.epoch);
    let artifact_id = Uuid::new_v4();
    let artifact_path = git_root
        .path()
        .join("artifacts")
        .join(artifact_id.to_string())
        .join("bundle.tar");
    std::fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
    let slow_bin = std::env::var("CARGO_BIN_EXE_musehub-pipeline-slow")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-pipeline-slow")
        });
    let mut slow = std::process::Command::new(&slow_bin);
    slow.env("MUSEHUB_PIPELINE_SOURCE_OID", &source_oid)
        .env("MUSEHUB_PIPELINE_ARTIFACT_PATH", &artifact_path)
        .env("MUSEHUB_PIPELINE_BARE_GIT", bare.to_str().unwrap())
        .env("MUSEHUB_PIPELINE_HOLD_SECS", "300")
        .env("MUSEHUB_PIPELINE_RUN_ID", run_id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut slow_child = slow.spawn().unwrap();
    let slow_pid = slow_child.id();
    renew.abort();
    tokio::time::sleep(Duration::from_secs(9)).await;

    let worker_b = Uuid::new_v4();
    let reclaimed = try_reclaim_expired_pipeline(&pool, run_id, worker_b)
        .await
        .expect("reclaim after TTL");
    assert!(reclaimed.epoch >= 2);

    let _ = std::process::Command::new("kill")
        .args(["-TERM", &slow_pid.to_string()])
        .status();
    let _ = slow_child.wait();

    std::env::set_var("MUSEHUB_PIPELINE_HOLD_SECS", "0");
    let reference = std::env::var("CARGO_BIN_EXE_musehub-pipeline-reference")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-pipeline-reference")
        });
    execute_pipeline_job(
        pool.clone(),
        git_root.path().to_path_buf(),
        git_root.path().join("artifacts"),
        reference,
        PipelineSupervisor::new(),
        worker_b,
        storage_key,
        run_id,
        job_id,
        org_id,
        repo_id,
        source_oid,
        "pr_validation".to_string(),
    )
    .await;

    let run_st: (String,) = sqlx::query_as("SELECT state FROM pipeline_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(run_st.0, "succeeded");
    let ready: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM artifacts WHERE pipeline_run_id = $1 AND state = 'ready'",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ready.0, 1);
}
