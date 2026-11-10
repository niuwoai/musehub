//! Deployment attempt_id: 409 on payload mismatch; cross-repo isolation; concurrent replay.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

async fn seed_idem_repo(
    pool: &sqlx::PgPool,
    git_root: &std::path::Path,
    slug: &str,
) -> (Uuid, Uuid, Uuid) {
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state)
         SELECT $1, o.id, $2, $3, $4, 'ready'
         FROM organizations o WHERE o.slug_normalized = 'demo'",
    )
    .bind(Uuid::new_v4())
    .bind(slug)
    .bind(slug)
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .unwrap();
    let (org_id, repo_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT org_id, id FROM repositories WHERE slug_normalized = $1")
            .bind(slug)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1,$2,$3,$4,'maintainer')
         ON CONFLICT (repo_id, principal_id) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(admin_id.0)
    .execute(pool)
    .await
    .unwrap();
    musehub_app::seed_repo_environment(pool, org_id, repo_id)
        .await
        .unwrap();
    let pipe_id = Uuid::new_v4();
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
    .execute(pool)
    .await
    .unwrap();
    let artifact_bytes = b"conflict-bundle";
    let sha_hex = Sha256::digest(artifact_bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    let artifact_dir = git_root
        .join("artifacts")
        .join(org_id.to_string())
        .join(repo_id.to_string());
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("bundle.tar"), artifact_bytes).unwrap();
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
    let release_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'0.0.1','v0.0.1',$4,$5,'published',$6)",
    )
    .bind(release_id)
    .bind(org_id)
    .bind(repo_id)
    .bind("b".repeat(40))
    .bind(pipe_id)
    .bind("d".repeat(64))
    .execute(pool)
    .await
    .unwrap();
    (org_id, repo_id, release_id)
}

#[tokio::test]
async fn attempt_id_conflict_on_different_release() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;

    let (org_id, repo_id, release_a) = seed_idem_repo(&pool, git_root.path(), "conflict-a").await;
    let release_b = Uuid::new_v4();
    let pipe_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','succeeded',
           (SELECT id FROM principals WHERE handle_normalized = 'admin' LIMIT 1),$5,'release_build')",
    )
    .bind(pipe_b)
    .bind(org_id)
    .bind(repo_id)
    .bind("c".repeat(40))
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, sha256, byte_length, storage_path, state)
         VALUES ($1,$2,$3,$4,'bundle.tar',$5,13,$6,'ready')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(pipe_b)
    .bind("e".repeat(64))
    .bind(git_root.path().to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
         VALUES ($1,$2,$3,'0.0.2','v0.0.2',$4,$5,'published',$6)",
    )
    .bind(release_b)
    .bind(org_id)
    .bind(repo_id)
    .bind("f".repeat(40))
    .bind(pipe_b)
    .bind("a".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    let attempt = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id)
         SELECT $1,$2,$3,e.id,$4,'compose-mock','succeeded',$5
         FROM environments e WHERE e.repo_id = $3 AND e.slug = 'test' LIMIT 1",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(release_a)
    .bind(attempt)
    .execute(&pool)
    .await
    .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/conflict-a/deployments")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "release_id": release_b,
                        "attempt_id": attempt,
                        "health_target": "mock://healthy"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn attempt_id_isolated_per_repo() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;

    let (_org_a, _repo_a, release_a) = seed_idem_repo(&pool, git_root.path(), "iso-a").await;
    let (_org_b, _repo_b, release_b) = seed_idem_repo(&pool, git_root.path(), "iso-b").await;
    let attempt = Uuid::new_v4();

    let (org_id, repo_a): (Uuid, Uuid) =
        sqlx::query_as("SELECT org_id, id FROM repositories WHERE slug_normalized = 'iso-a'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO deployments (id, org_id, repo_id, environment_id, release_id, adapter_kind, state, attempt_id)
         SELECT $1,$2,$3,e.id,$4,'compose-mock','succeeded',$5
         FROM environments e WHERE e.repo_id = $3 AND e.slug = 'test' LIMIT 1",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_a)
    .bind(release_a)
    .bind(attempt)
    .execute(&pool)
    .await
    .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/iso-b/deployments")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "release_id": release_b,
                        "attempt_id": attempt,
                        "health_target": "mock://healthy"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn concurrent_attempt_inserts_one_row() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;

    let (_org_id, repo_id, release_id) =
        seed_idem_repo(&pool, git_root.path(), "conc-deploy").await;
    let attempt = Uuid::new_v4();
    let body = serde_json::json!({
        "release_id": release_id,
        "attempt_id": attempt,
        "health_target": "mock://healthy"
    })
    .to_string();

    let send = |cookie: String| {
        let app = app.clone();
        let body = body.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/conc-deploy/deployments")
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    let (a, b) = tokio::join!(send(cookie.clone()), send(cookie));
    let ok_status = |s: StatusCode| s == StatusCode::CREATED || s == StatusCode::OK;
    assert!(ok_status(a.status()));
    assert!(ok_status(b.status()));

    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM deployments WHERE repo_id = $1 AND attempt_id = $2")
            .bind(repo_id)
            .bind(attempt)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 1);
}
