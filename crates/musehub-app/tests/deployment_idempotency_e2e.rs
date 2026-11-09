//! Deployment POST replays the same row when attempt_id matches (restart-safe).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn deployment_start_replays_attempt_id() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"idem-deploy","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let rel_id = Uuid::new_v4();
    let pipe_id = Uuid::new_v4();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let repo_row: (Uuid, Uuid) =
        sqlx::query_as("SELECT org_id, id FROM repositories WHERE slug_normalized = 'idem-deploy'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let (org_id, repo_id) = repo_row;
    musehub_app::seed_repo_environment(&pool, org_id, repo_id)
        .await
        .unwrap();
    let artifact_bytes = b"idem-test-bundle";
    let sha256 = Sha256::digest(artifact_bytes);
    let sha_hex = sha256
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
    let manifest_sha = "c".repeat(64);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id)
    .bind(repo_id)
    .bind("a".repeat(40))
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
    .bind("b".repeat(40))
    .bind(pipe_id)
    .bind(&manifest_sha)
    .execute(&pool)
    .await
    .unwrap();

    let attempt = Uuid::new_v4();
    let body = serde_json::json!({
        "release_id": rel_id,
        "health_target": "mock://healthy",
        "attempt_id": attempt
    });
    let post = || {
        let app = app.clone();
        let admin_cookie = admin_cookie.clone();
        let body = body.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/idem-deploy/deployments")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    let first = post().await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_json: serde_json::Value =
        serde_json::from_slice(&first.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let dep_id = first_json["id"].as_str().unwrap().to_string();
    assert_eq!(
        first_json["attempt_id"].as_str().unwrap(),
        attempt.to_string()
    );

    let second = post().await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_json: serde_json::Value =
        serde_json::from_slice(&second.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(second_json["id"].as_str().unwrap(), dep_id);
    assert_eq!(second_json["state"].as_str().unwrap(), "succeeded");

    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM deployments WHERE repo_id = $1 AND attempt_id = $2")
            .bind(repo_id)
            .bind(attempt)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 1);
}
