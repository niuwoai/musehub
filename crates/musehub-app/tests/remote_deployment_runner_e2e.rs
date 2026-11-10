//! Remote deploy executor: API enqueues; worker claims and runs compose adapter.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::execute_deployment_job;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

const RUNNER_PASS: &str = "Remote-Deploy-Member-12!";

#[tokio::test]
async fn remote_worker_claims_and_completes_deployment() {
    std::env::set_var("MUSEHUB_DEPLOY_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_COMPOSE_ADAPTER_MODE", "mock");
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
                        "handle": "remote-deploy",
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
                .body(Body::from(r#"{"slug":"remote-deploy","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_row: (Uuid, Uuid) = sqlx::query_as(
        "SELECT org_id, id FROM repositories WHERE slug_normalized = 'remote-deploy'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let (org_id, repo_id) = repo_row;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/remote-deploy/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "remote-deploy",
                        "role": "developer"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    musehub_app::seed_repo_environment(&pool, org_id, repo_id)
        .await
        .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let rel_id = Uuid::new_v4();
    let pipe_id = Uuid::new_v4();
    let artifact_bytes = b"remote-deploy-bundle";
    let sha_hex = Sha256::digest(artifact_bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    let artifact_dir = git_root
        .path()
        .join("artifacts")
        .join(org_id.to_string())
        .join(repo_id.to_string());
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("bundle.tar"), artifact_bytes).unwrap();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id)
    .bind(repo_id)
    .bind("d".repeat(40))
    .bind(admin_id.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, sha256, byte_length, storage_path, state)
         VALUES ($1,$2,$3,$4,'bundle.tar',$5,$6,$7,'ready')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(pipe_id)
    .bind(&sha_hex)
    .bind(artifact_bytes.len() as i64)
    .bind(artifact_dir.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'0.0.1','v0.0.1',$4,$5,'published',$6)",
    )
    .bind(rel_id)
    .bind(org_id)
    .bind(repo_id)
    .bind("e".repeat(40))
    .bind(pipe_id)
    .bind("f".repeat(64))
    .execute(&pool)
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
                        "name": "deploy-runner",
                        "org": "demo",
                        "scopes": ["deploy:execute", "repo:read"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_id],
                        "runner_handle": "remote-deploy"
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
    let attempt_id = Uuid::new_v4();
    let dep_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/remote-deploy/deployments")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "release_id": rel_id,
                        "health_target": "mock://healthy",
                        "attempt_id": attempt_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dep_resp.status(), StatusCode::ACCEPTED);
    let dep_json: serde_json::Value =
        serde_json::from_slice(&dep_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(dep_json["state"], "queued");
    let deployment_id = Uuid::parse_str(dep_json["id"].as_str().unwrap()).unwrap();
    let runner_id = Uuid::new_v4();
    let claim = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/runner/deployments/claim")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-musehub-runner-id", runner_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(claim.status(), StatusCode::OK);
    let claimed: serde_json::Value =
        serde_json::from_slice(&claim.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(claimed["deployment_id"], deployment_id.to_string());
    let epoch = claimed["lease_epoch"].as_i64().unwrap();
    let compose_mock = common::compose_mock_bin();
    let compose_runner = common::compose_runner_bin();
    execute_deployment_job(
        &pool,
        deployment_id,
        runner_id,
        epoch,
        compose_mock,
        compose_runner,
        false,
        rel_id,
    )
    .await
    .unwrap();
    let state: (String,) = sqlx::query_as("SELECT state FROM deployments WHERE id = $1")
        .bind(deployment_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "succeeded");
    std::env::remove_var("MUSEHUB_DEPLOY_EXECUTOR");
    std::env::remove_var("MUSEHUB_COMPOSE_ADAPTER_MODE");
}
