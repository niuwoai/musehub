//! Product deployment rollback REST (restore succeeded deployment row).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn rollback_restores_prior_succeeded_deployment() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"rollback-demo","description":""}"#))
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
    let repo_row: (Uuid, Uuid) = sqlx::query_as(
        "SELECT org_id, id FROM repositories WHERE slug_normalized = 'rollback-demo'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let (org_id, repo_id) = repo_row;
    musehub_app::seed_repo_environment(&pool, org_id, repo_id)
        .await
        .unwrap();

    let artifact_bytes = b"rollback-test-bundle";
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
    let commit_oid = "d".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&commit_oid)
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
    let manifest_sha = "e".repeat(64);
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'0.0.1-rb','v0.0.1-rb',$4,$5,'published',$6)",
    )
    .bind(rel_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&commit_oid)
    .bind(pipe_id)
    .bind(&manifest_sha)
    .execute(&pool)
    .await
    .unwrap();

    let dep: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/rollback-demo/deployments")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "release_id": rel_id,
                            "health_target": "mock://healthy"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(dep["state"], "succeeded");
    let first_id = dep["id"].as_str().unwrap();

    let rollback: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/rollback-demo/deployments/rollback")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "target_deployment_id": first_id,
                            "reason": "ci-rollback-smoke"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(rollback["state"], "succeeded");
    assert_eq!(rollback["release_id"], rel_id.to_string());
    let rb_id = rollback["id"].as_str().unwrap();
    assert_ne!(rb_id, first_id, "rollback must create a new deployment row");
    assert!(
        rollback["health_summary"]["rollback"].as_bool() == Some(true),
        "fresh adapter summary"
    );
    let row: (Option<Uuid>,) =
        sqlx::query_as("SELECT rollback_of_id FROM deployments WHERE id = $1")
            .bind(Uuid::parse_str(rb_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, Some(Uuid::parse_str(first_id).unwrap()));

    let audit_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/audit/events?resource_type=deployment")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audit_resp.status(), StatusCode::OK);
    let audit: Vec<serde_json::Value> =
        serde_json::from_slice(&audit_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert!(audit.iter().any(|e| e["action"] == "deployment.rollback"));
}
