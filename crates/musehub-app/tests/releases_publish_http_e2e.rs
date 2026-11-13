//! POST /releases + /publish: Maintainer gate (docs/01-product-requirements.md §4) and pipeline rules.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

const MEMBER_PASS: &str = "Rel-Publish-Member-12!";

async fn seed_succeeded_pipeline(
    pool: &sqlx::PgPool,
    git_root: &std::path::Path,
    org_id: Uuid,
    repo_id: Uuid,
    admin_id: Uuid,
) -> (Uuid, String) {
    let oid = "e".repeat(40);
    let pipe_id = Uuid::new_v4();
    let artifact_bytes = b"rel-publish-bundle";
    let sha256 = Sha256::digest(artifact_bytes);
    let sha_hex: String = sha256.iter().map(|b| format!("{:02x}", b)).collect();
    let artifact_dir = git_root
        .join("artifacts")
        .join(org_id.to_string())
        .join(repo_id.to_string());
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("bundle.tar"), artifact_bytes).unwrap();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'release','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&oid)
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .execute(pool)
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
    .execute(pool)
    .await
    .unwrap();
    (pipe_id, oid)
}

#[tokio::test]
async fn release_create_and_publish_requires_maintainer_and_succeeded_pipeline() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &boot).await;
    let admin = common::login_cookie(&app, "admin", &boot).await;
    let repo = format!("rel-pub-{}", Uuid::new_v4().simple());
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "slug": repo }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE org_id = $1 AND slug = $2")
            .bind(org_id.0)
            .bind(&repo)
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let (pipe_id, oid) =
        seed_succeeded_pipeline(&pool, git_root.path(), org_id.0, repo_id.0, admin_id.0).await;

    let reader = format!("relread{}", &Uuid::new_v4().simple().to_string()[..6]);
    let developer = format!("reldev{}", &Uuid::new_v4().simple().to_string()[..6]);
    let maint = format!("relmnt{}", &Uuid::new_v4().simple().to_string()[..6]);
    let outsider = format!("relpubx{}", &Uuid::new_v4().simple().to_string()[..6]);
    for handle in [
        reader.as_str(),
        developer.as_str(),
        maint.as_str(),
        outsider.as_str(),
    ] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/members")
                    .header(header::COOKIE, &admin)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "handle": handle, "password": MEMBER_PASS, "role": "member" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    for (handle, role) in [
        (reader.as_str(), "reader"),
        (developer.as_str(), "developer"),
        (maint.as_str(), "maintainer"),
    ] {
        let grant = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/orgs/demo/repos/{repo}/collaborators"))
                    .header(header::COOKIE, &admin)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "handle": handle, "role": role }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            grant.status() == StatusCode::CREATED || grant.status() == StatusCode::NO_CONTENT,
            "grant {}",
            grant.status()
        );
    }

    let reader_cookie = common::login_cookie(&app, &reader, MEMBER_PASS).await;
    let developer_cookie = common::login_cookie(&app, &developer, MEMBER_PASS).await;
    let maint_cookie = common::login_cookie(&app, &maint, MEMBER_PASS).await;
    let outsider_cookie = common::login_cookie(&app, &outsider, MEMBER_PASS).await;

    let body = json!({
        "version": "1.0.0",
        "tag_name": "v1.0.0",
        "commit_oid": oid,
        "pipeline_run_id": pipe_id,
    });
    let releases_uri = format!("/api/v1/orgs/demo/repos/{repo}/releases");

    for cookie in [&reader_cookie, &developer_cookie, &outsider_cookie] {
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&releases_uri)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            denied.status() == StatusCode::FORBIDDEN || denied.status() == StatusCode::NOT_FOUND,
            "expected forbidden for non-maintainer, got {}",
            denied.status()
        );
    }

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&releases_uri)
                .header(header::COOKIE, &maint_cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let draft: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(draft["state"], "draft");
    let release_id = draft["id"].as_str().unwrap();

    let pub_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/releases/{release_id}/publish"
                ))
                .header(header::COOKIE, &maint_cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pub_resp.status(), StatusCode::OK);
    let published: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(pub_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(published["state"], "published");
    assert!(published["manifest_sha256"].as_str().unwrap().len() == 64);

    let publish_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/releases/{release_id}/publish"
                ))
                .header(header::COOKIE, &reader_cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(publish_denied.status(), StatusCode::FORBIDDEN);

    let failed_pipe = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'release','failed',$5,$6,'release_build')",
    )
    .bind(failed_pipe)
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(&oid)
    .bind(admin_id.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let bad_body = json!({
        "version": "1.0.1",
        "tag_name": "v1.0.1",
        "commit_oid": oid,
        "pipeline_run_id": failed_pipe,
    });
    let blocked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&releases_uri)
                .header(header::COOKIE, &maint_cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bad_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::BAD_REQUEST);
}
