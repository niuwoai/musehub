//! S1 slice: git browse, issues, PR anchors/sync, SSH keys.

mod common;

use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

fn temp_ed25519_public() -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = dir.path().join("id_ed25519");
    let status = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            key.to_str().expect("path"),
            "-N",
            "",
            "-q",
        ])
        .status()
        .expect("ssh-keygen");
    assert!(status.success());
    std::fs::read_to_string(key.with_extension("pub")).expect("pub")
}

use common::s1_git_push::push_branch;

fn push_extra_commits(git_root: &std::path::Path, storage_key: Uuid, branch: &str, n: usize) {
    for i in 0..n {
        push_branch(
            git_root,
            storage_key,
            branch,
            &format!("log-{i}.txt"),
            format!("c{i}").as_bytes(),
        );
    }
}

#[tokio::test]
async fn s1_collaboration_rest_slice() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;

    let repo = format!("s1-{}", Uuid::new_v4().simple());
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "slug": repo }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = $1")
        .bind(&repo)
        .fetch_one(&pool)
        .await
        .unwrap();
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(repo_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
    push_extra_commits(&git_root, storage_key.0, "hist-log", 4);
    let feature_head = push_branch(
        &git_root,
        storage_key.0,
        "feature/s1",
        "feature.txt",
        b"line one\nline two\n",
    );

    let commits = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/commits?ref=hist-log&limit=2"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(commits.status(), StatusCode::OK);
    let commits_json = body_json(commits).await;
    assert_eq!(commits_json["items"].as_array().unwrap().len(), 2);
    assert_eq!(commits_json["has_more"], true);
    let _head_oid = commits_json["commit_oid"].as_str().unwrap().to_string();

    let log = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/tree?commit={feature_head}&path="
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(log.status(), StatusCode::OK);
    let tree = body_json(log).await;
    assert!(tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["path"] == "feature.txt"));

    let bad_path = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/tree?commit=main&path=..%2Fetc"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_path.status(), StatusCode::BAD_REQUEST);

    let blob = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/blob?commit={feature_head}&path=feature.txt"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blob.status(), StatusCode::OK);
    let blob_json = body_json(blob).await;
    assert!(blob_json["text"].as_str().unwrap().contains("line one"));

    let reviewer = format!("s1rev{}", &Uuid::new_v4().simple().to_string()[..10]);
    let reviewer_pass = "S1-Reviewer-Strong-12!";
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/members")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "handle": reviewer, "password": reviewer_pass, "role": "member" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/orgs/demo/repos/{repo}/collaborators"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "handle": reviewer, "role": "maintainer" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    let nogrant = format!("nogrant{}", &Uuid::new_v4().simple().to_string()[..8]);
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/members")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "handle": nogrant,
                            "password": "S1-NoGrant-Strong-12!",
                            "role": "member"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "title": "s1 issue", "body": "hello" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(issue_resp.status(), StatusCode::CREATED);
    let issue_number = body_json(issue_resp).await["number"].as_i64().unwrap();

    let nogrant_assignee = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues/{issue_number}"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "assignee_handle": nogrant }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(nogrant_assignee.status(), StatusCode::BAD_REQUEST);

    let bad_labels = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues/{issue_number}"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "labels": ["bad label!"] }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_labels.status(), StatusCode::BAD_REQUEST);

    let patch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues/{issue_number}"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "labels": ["bug", "p0"], "assignee_handle": reviewer }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch.status(), StatusCode::OK);

    let unknown_assignee = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/issues?assignee=not-a-real-user"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown_assignee.status(), StatusCode::BAD_REQUEST);

    let issue2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "title": "s1 issue two", "body": "two" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(issue2.status(), StatusCode::CREATED);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues?limit=1"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = body_json(list).await;
    assert!(list_json["has_more"].as_bool().unwrap());

    let pr_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/pull-requests"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "title": "s1 pr",
                        "source_branch": "feature/s1",
                        "target_branch": "main",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pr_create.status(), StatusCode::CREATED);
    let pr_number = body_json(pr_create).await["number"].as_i64().unwrap();

    let detail = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let pr_json = body_json(detail).await;
    let row_version = pr_json["row_version"].as_i64().unwrap();

    let self_review = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/reviews"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "decision": "approve" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(self_review.status(), StatusCode::BAD_REQUEST);

    let bad_anchor = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/comments"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "body": "nope",
                        "anchor": {
                            "path": "feature.txt",
                            "line": 9999,
                            "side": "RIGHT",
                            "pr_revision_id": row_version
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_anchor.status(), StatusCode::BAD_REQUEST);

    let good_anchor = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/comments"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "body": "on diff",
                        "anchor": {
                            "path": "feature.txt",
                            "line": 1,
                            "side": "RIGHT",
                            "pr_revision_id": row_version
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(good_anchor.status(), StatusCode::CREATED);

    let stale_revision = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/comments"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "body": "stale revision",
                        "anchor": {
                            "path": "feature.txt",
                            "line": 1,
                            "side": "RIGHT",
                            "pr_revision_id": row_version.saturating_sub(1)
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_revision.status(), StatusCode::BAD_REQUEST);

    let stale_sync = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/sync"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "expected_row_version": 0 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_sync.status(), StatusCode::BAD_REQUEST);

    push_branch(
        &git_root,
        storage_key.0,
        "feature/s1",
        "feature.txt",
        b"line one\nline two\nline three\n",
    );
    let sync_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/sync"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "expected_row_version": row_version }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_ok.status(), StatusCode::OK);
    let sync_body = body_json(sync_ok).await;
    let bumped_row = sync_body["row_version"]
        .as_i64()
        .expect("sync returns row_version");
    assert!(bumped_row > row_version);

    let sync_noop = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/sync"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "expected_row_version": bumped_row }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_noop.status(), StatusCode::OK);
    let noop_body = body_json(sync_noop).await;
    assert_eq!(noop_body["row_version"], bumped_row);

    let comments = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/comments"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let thread = body_json(comments).await;
    assert!(thread
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["outdated"] == true));

    let reviewer_cookie = common::login_cookie(&app, &reviewer, reviewer_pass).await;
    let approve = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/reviews"
                ))
                .header(header::COOKIE, &reviewer_cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "decision": "approve" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approve.status(), StatusCode::CREATED);

    let pub_key = temp_ed25519_public();
    let ssh = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ssh-keys")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "name": "ci-test", "public_key": pub_key }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ssh.status(), StatusCode::CREATED);
    let key_id = body_json(ssh).await["id"].as_str().unwrap().to_string();

    let dup = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ssh-keys")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "name": "ci-dup", "public_key": pub_key }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::BAD_REQUEST);

    let revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/ssh-keys/{key_id}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);

    let readd = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ssh-keys")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "name": "ci-readd", "public_key": pub_key }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(readd.status(), StatusCode::CREATED);
    let readd_id = body_json(readd).await["id"].as_str().unwrap().to_string();

    let steal_revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/ssh-keys/{readd_id}"))
                .header(header::COOKIE, &reviewer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(steal_revoke.status(), StatusCode::NOT_FOUND);
}
