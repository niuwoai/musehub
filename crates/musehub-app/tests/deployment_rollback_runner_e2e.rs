//! Two-release compose-runner rollback with live HTTP body (commit oid) + trace/digest.

mod common;

use std::path::PathBuf;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

fn docker_ok() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} must be set by scripts/verify-pipeline-oci-product-e2e.sh")
    })
}

async fn seed_oci_release(
    pool: &sqlx::PgPool,
    git_root: &std::path::Path,
    slug: &str,
    commit_oid: &str,
    oci_ref: &str,
    version: &str,
) -> (Uuid, Uuid, Uuid) {
    let admin_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(pool)
            .await
            .unwrap();
    let (org_id, repo_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT org_id, id FROM repositories WHERE slug_normalized = $1")
            .bind(slug)
            .fetch_one(pool)
            .await
            .unwrap();
    let rel_id = Uuid::new_v4();
    let pipe_id = Uuid::new_v4();
    let artifact_bytes = format!("bundle-{version}").into_bytes();
    let sha_hex = Sha256::digest(&artifact_bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    let artifact_dir = git_root
        .join("artifacts")
        .join(org_id.to_string())
        .join(repo_id.to_string())
        .join(pipe_id.to_string());
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let bundle_path = artifact_dir.join("bundle.tar");
    std::fs::write(&bundle_path, &artifact_bytes).unwrap();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','succeeded',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(commit_oid)
    .bind(admin_id.0)
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
    let oci_ref_bytes = oci_ref.as_bytes();
    let oci_sha = Sha256::digest(oci_ref_bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    std::fs::write(artifact_dir.join("oci-image.ref"), oci_ref).unwrap();
    sqlx::query(
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, kind, sha256, byte_length, storage_path, state, oci_image_ref)
         VALUES ($1,$2,$3,$4,'oci-image.ref','oci_image',$5,$6,$7,'ready',$8)",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(pipe_id)
    .bind(&oci_sha)
    .bind(oci_ref_bytes.len() as i64)
    .bind(artifact_dir.to_string_lossy().as_ref())
    .bind(oci_ref)
    .execute(pool)
    .await
    .unwrap();
    let manifest_sha = Sha256::digest(format!("{commit_oid}:{oci_ref}").as_bytes())
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    sqlx::query(
        "INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256, oci_image_ref)
         VALUES ($1,$2,$3,$4,$5,$6,$7,'published',$8,$9)",
    )
    .bind(rel_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(version)
    .bind(format!("v{version}"))
    .bind(commit_oid)
    .bind(pipe_id)
    .bind(&manifest_sha)
    .bind(oci_ref)
    .execute(pool)
    .await
    .unwrap();
    (rel_id, repo_id, pipe_id)
}

async fn deploy_release(
    app: &axum::Router,
    cookie: &str,
    slug: &str,
    release_id: Uuid,
) -> serde_json::Value {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{slug}/deployments"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(
                    serde_json::json!({ "release_id": release_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "deploy failed: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

async fn http_body(url: &str) -> String {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap();
    for _ in 0..40 {
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().is_success() {
                return resp.text().await.unwrap_or_default().trim().to_string();
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("HTTP probe failed for {url}");
}

#[tokio::test]
async fn runner_two_release_rollback_http_and_trace() {
    if !docker_ok() {
        eprintln!("skip: docker unavailable");
        return;
    }
    if std::env::var("MUSEHUB_REQUIRE_PIPELINE_OCI")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!("skip: MUSEHUB_REQUIRE_PIPELINE_OCI not set");
        return;
    }
    let oid1 = required_env("MUSEHUB_TEST_OID1");
    let oid2 = required_env("MUSEHUB_TEST_OID2");
    let ref1 = required_env("MUSEHUB_TEST_REF1");
    let ref2 = required_env("MUSEHUB_TEST_REF2");
    assert_eq!(oid1.len(), 40);
    assert_eq!(oid2.len(), 40);
    assert!(ref1.contains("@sha256:"));
    assert!(ref2.contains("@sha256:"));
    assert_ne!(ref1, ref2);

    std::env::set_var("MUSEHUB_COMPOSE_ADAPTER_MODE", "runner");
    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/musehub-compose-runner");
    let mock =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/musehub-compose-mock");
    std::env::set_var(
        "CARGO_BIN_EXE_musehub-compose-runner",
        runner.canonicalize().unwrap(),
    );
    std::env::set_var(
        "CARGO_BIN_EXE_musehub-compose-mock",
        mock.canonicalize().unwrap(),
    );
    std::env::remove_var("CARGO_BIN_EXE_musehub_compose_runner");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"oci-rb-two","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let (org_id, repo_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT org_id, id FROM repositories WHERE slug_normalized = 'oci-rb-two'")
            .fetch_one(&pool)
            .await
            .unwrap();
    musehub_app::seed_repo_environment(&pool, org_id, repo_id)
        .await
        .unwrap();

    let (rel_a, _, _pipe_a) =
        seed_oci_release(&pool, git_root.path(), "oci-rb-two", &oid1, &ref1, "0.0.1").await;
    let (rel_b, _, _pipe_b) =
        seed_oci_release(&pool, git_root.path(), "oci-rb-two", &oid2, &ref2, "0.0.2").await;
    let dep_a = deploy_release(&app, &cookie, "oci-rb-two", rel_a).await;
    assert_eq!(
        dep_a["state"], "succeeded",
        "dep_a health={:?}",
        dep_a["health_summary"]
    );
    assert_eq!(dep_a["adapter_kind"], "compose-runner");
    let url_a = dep_a["health_summary"]["target"].as_str().unwrap();
    assert_eq!(http_body(url_a).await, oid1);

    let dep_b = deploy_release(&app, &cookie, "oci-rb-two", rel_b).await;
    assert_eq!(dep_b["state"], "succeeded");
    let url_b = dep_b["health_summary"]["target"].as_str().unwrap();
    assert_eq!(http_body(url_b).await, oid2);

    let dep_a_id = dep_a["id"].as_str().unwrap();
    let rollback = serde_json::from_slice::<serde_json::Value>(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/oci-rb-two/deployments/rollback")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "target_deployment_id": dep_a_id,
                            "reason": "oci-two-release-rollback"
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
    assert_eq!(
        rollback["state"], "succeeded",
        "rollback health={:?}",
        rollback["health_summary"]
    );
    assert_eq!(rollback["release_id"], rel_a.to_string());
    let url_rb = rollback["health_summary"]["target"].as_str().unwrap();
    assert_eq!(http_body(url_rb).await, oid1);
    assert_eq!(
        rollback["health_summary"]["applied_oci_digest"]
            .as_str()
            .unwrap(),
        ref1.split('@').nth(1).unwrap()
    );
    assert_eq!(
        rollback["health_summary"]["verified_source_oid"]
            .as_str()
            .unwrap(),
        oid1
    );

    let rb_id = rollback["id"].as_str().unwrap();
    let trace: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/v1/orgs/demo/repos/oci-rb-two/trace/deployments/{rb_id}"
                    ))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
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
    assert_eq!(trace["commit_oid"].as_str().unwrap(), oid1);
    assert_eq!(trace["verified_source_oid"].as_str().unwrap(), oid1);
    assert!(trace["release_oci_image_ref"]
        .as_str()
        .is_some_and(|r| r == ref1));
}
