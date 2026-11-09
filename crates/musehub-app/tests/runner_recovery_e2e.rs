//! Cancel, lease TTL reclaim, stale epoch finalize, revoked PAT renew.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::lease::{renew_pipeline_work_lease, try_reclaim_expired_pipeline};
use tower::ServiceExt;
use uuid::Uuid;

const MEMBER_PASS: &str = "Runner-Recovery-Member-1!";

async fn mint_runner_pat(app: &axum::Router, admin_cookie: &str, repo_id: Uuid) -> (String, Uuid) {
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "name": "recovery-runner",
                        "org": "demo",
                        "scopes": ["pipeline:execute", "repo:read"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_id],
                        "runner_handle": "recovery-runner"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (
        body["token"].as_str().unwrap().to_string(),
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn runner_recovery_matrix() {
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
                        "handle": "recovery-runner",
                        "password": MEMBER_PASS,
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
                .body(Body::from(r#"{"slug":"recv-pipe","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM repositories WHERE slug = 'recv-pipe'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/recv-pipe/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    r#"{"handle":"recovery-runner","role":"developer"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (token, token_id) = mint_runner_pat(&app, &admin_cookie, repo_id).await;
    let run_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let org_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug = 'demo'")
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
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, run_kind, lease_holder, lease_epoch)
         VALUES ($1,$2,$3,$4,'product-ci','running',$5,$6,'pr_validation',$7,1)",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind("a".repeat(40))
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','running')",
    )
    .bind(job_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap();

    let stale_worker = Uuid::new_v4();
    assert!(
        !musehub_app::try_publish_stale_pipeline_success_for_test(
            &pool,
            run_id,
            job_id,
            stale_worker,
            0
        )
        .await
    );
    let state: (String,) = sqlx::query_as("SELECT state FROM pipeline_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "running");

    sqlx::query(
        "UPDATE pipeline_runs SET lease_until = CURRENT_TIMESTAMP - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap();
    let new_worker = Uuid::new_v4();
    let reclaim = try_reclaim_expired_pipeline(&pool, run_id, new_worker)
        .await
        .unwrap();
    assert!(reclaim.epoch > 1);
    assert!(
        !musehub_app::try_publish_stale_pipeline_success_for_test(
            &pool,
            run_id,
            job_id,
            stale_worker,
            1
        )
        .await
    );

    let runner_a = Uuid::new_v4();
    sqlx::query(
        "UPDATE pipeline_runs SET state = 'running', lease_holder = $2, lease_until = CURRENT_TIMESTAMP + INTERVAL '1 hour', lease_epoch = 5 WHERE id = $1",
    )
    .bind(run_id)
    .bind(runner_a)
    .execute(&pool)
    .await
    .unwrap();
    let cancel = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/recv-pipe/pipeline-runs/{run_id}/cancel"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::OK);
    let cancel_st: (String,) = sqlx::query_as("SELECT state FROM pipeline_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cancel_st.0, "cancel_requested");

    sqlx::query(
        "UPDATE pipeline_runs SET state = 'running', cancel_requested = FALSE, lease_holder = $2,
         lease_until = CURRENT_TIMESTAMP + INTERVAL '1 hour', lease_epoch = 6 WHERE id = $1",
    )
    .bind(run_id)
    .bind(runner_a)
    .execute(&pool)
    .await
    .unwrap();
    assert!(renew_pipeline_work_lease(&pool, run_id, runner_a, 6)
        .await
        .is_ok());
    sqlx::query("UPDATE access_tokens SET revoked_at = CURRENT_TIMESTAMP WHERE id = $1")
        .bind(token_id)
        .execute(&pool)
        .await
        .unwrap();
    let renew_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/runner/pipeline-runs/{run_id}/lease/renew"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-musehub-runner-id", runner_a.to_string())
                .body(Body::from(r#"{"lease_epoch":6}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        renew_resp.status() == StatusCode::UNAUTHORIZED
            || renew_resp.status() == StatusCode::FORBIDDEN
    );

    let _ = token;
}
