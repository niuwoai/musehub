//! Concurrent PR comment + sync must not leave anchored comments outdated=false past row_version.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use common::s1_git_push::push_branch;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn pr_anchor_comment_sync_concurrency_invariant() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;
    let repo = format!("race-{}", Uuid::new_v4().simple());
    assert_eq!(
        app.clone()
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
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let storage_key: (Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug_normalized = $1")
            .bind(&repo)
            .fetch_one(&pool)
            .await
            .unwrap();
    push_branch(
        &git_root,
        storage_key.0,
        "feature/race",
        "race.txt",
        b"v1\n",
    );
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
                        "title": "race",
                        "source_branch": "feature/race",
                        "target_branch": "main",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pr_number = body_json(pr_create).await["number"].as_i64().unwrap();

    for round in 0..25 {
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
        let row_version = body_json(detail).await["row_version"].as_i64().unwrap();
        if round % 2 == 0 {
            push_branch(
                &git_root,
                storage_key.0,
                "feature/race",
                "race.txt",
                format!("v{round}\n").as_bytes(),
            );
        }
        let comment_uri =
            format!("/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/comments");
        let sync_uri = format!("/api/v1/orgs/demo/repos/{repo}/pull-requests/{pr_number}/sync");
        let cookie_c = cookie.clone();
        let app_c = app.clone();
        let comment_fut = async move {
            app_c
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(&comment_uri)
                        .header(header::COOKIE, &cookie_c)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({
                                "body": format!("c{round}"),
                                "anchor": {
                                    "path": "race.txt",
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
                .unwrap()
        };
        let cookie_s = cookie.clone();
        let app_s = app.clone();
        let sync_fut = async move {
            app_s
                .oneshot(
                    Request::builder()
                        .method("PATCH")
                        .uri(&sync_uri)
                        .header(header::COOKIE, &cookie_s)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({ "expected_row_version": row_version }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap()
        };
        let (comment_res, sync_res) = tokio::join!(comment_fut, sync_fut);
        assert!(
            comment_res.status().is_success()
                || comment_res.status() == StatusCode::BAD_REQUEST
                || comment_res.status() == StatusCode::NOT_FOUND
        );
        assert!(sync_res.status().is_success() || sync_res.status() == StatusCode::BAD_REQUEST);

        let detail2 = app
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
        let current_rev = body_json(detail2).await["row_version"].as_i64().unwrap();
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
        let list = body_json(comments).await;
        for c in list.as_array().unwrap() {
            let Some(anchor) = c.get("anchor").and_then(|a| a.as_object()) else {
                continue;
            };
            let Some(rev) = anchor.get("pr_revision_id").and_then(|v| v.as_i64()) else {
                continue;
            };
            let outdated = c.get("outdated").and_then(|v| v.as_bool()).unwrap_or(false);
            if rev < current_rev {
                assert!(
                    outdated,
                    "anchored comment at revision {rev} must be outdated when PR is at {current_rev}"
                );
            }
        }
    }
}
