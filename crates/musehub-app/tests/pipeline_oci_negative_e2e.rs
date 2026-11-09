//! API negative paths for pipeline-bound OCI (no Docker required).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn publish_rejects_tampered_oci_image_ref() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    std::env::set_var("MUSEHUB_REQUIRE_PIPELINE_OCI", "0");
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin = common::login_cookie(&app, "admin", &bootstrap).await;
    let principal: (Uuid,) = sqlx::query_as("SELECT id FROM principals WHERE handle = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin)
                .body(Body::from(r#"{"slug":"oci-neg","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let pipe_id = Uuid::new_v4();
    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'oci-neg'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let oid = "a".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'release','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(principal.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let art_id = Uuid::new_v4();
    let oci = format!(
        "127.0.0.1:5000/musehub/pipeline-{}@sha256:{}",
        repo_id.0,
        "b".repeat(64)
    );
    sqlx::query(
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, kind, sha256, byte_length, storage_path, state, oci_image_ref)
         VALUES ($1,$2,$3,$4,'oci-image.ref','oci_image',$5,10,'/tmp','ready',$6)",
    )
    .bind(art_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(pipe_id)
    .bind("c".repeat(64))
    .bind(&oci)
    .execute(&pool)
    .await
    .unwrap();

    let release_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state)
         VALUES ($1,$2,$3,'0.0.1','v0.0.1',$4,$5,'draft')",
    )
    .bind(release_id)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(pipe_id)
    .execute(&pool)
    .await
    .unwrap();

    let tampered = format!(
        "127.0.0.1:5000/musehub/pipeline-{}@sha256:{}",
        repo_id.0,
        "d".repeat(64)
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/oci-neg/releases/{release_id}/publish"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin)
                .body(Body::from(
                    serde_json::json!({ "oci_image_ref": tampered }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
