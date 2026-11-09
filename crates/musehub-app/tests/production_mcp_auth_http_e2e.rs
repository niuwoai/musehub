//! Bearer-only MCP HTTP gate: session id is not a credential.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use musehub_app::mcp::parse_bearer_token;
use serde_json::json;
use tokio::net::TcpListener;
use tower::ServiceExt as TowerServiceExt;

struct Fixture {
    url: String,
    token: String,
    pool: sqlx::PgPool,
}

async fn setup() -> Fixture {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"mcp-http","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(pat_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let token = pat_json["token"].as_str().unwrap().to_string();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    Fixture {
        url: format!("http://{addr}/mcp"),
        token,
        pool,
    }
}

async fn initialize_post(url: &str, bearer: Option<&str>, session_id: Option<&str>) -> StatusCode {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "auth-test", "version": "0" }
        }
    });
    let client = reqwest::Client::new();
    let mut req = client
        .post(url)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(token) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(sid) = session_id {
        req = req.header("mcp-session-id", sid);
    }
    req.body(body.to_string())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .expect("post")
        .status()
}

#[tokio::test]
async fn mcp_session_id_without_bearer_is_unauthorized() {
    let fx = setup().await;
    assert_eq!(
        initialize_post(&fx.url, Some(&fx.token), None).await,
        StatusCode::OK
    );
    assert_eq!(
        initialize_post(&fx.url, None, Some("stolen-session-id")).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn mcp_revoked_pat_fails_immediately() {
    let fx = setup().await;
    assert_eq!(
        initialize_post(&fx.url, Some(&fx.token), None).await,
        StatusCode::OK
    );
    sqlx::query("UPDATE access_tokens SET revoked_at = CURRENT_TIMESTAMP WHERE token_hash = $1")
        .bind(musehub_app::pat::hash_pat(&fx.token))
        .execute(&fx.pool)
        .await
        .unwrap();
    assert_eq!(
        initialize_post(&fx.url, Some(&fx.token), None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn mcp_foreign_session_header_with_valid_bearer_succeeds() {
    let fx = setup().await;
    assert_eq!(
        initialize_post(&fx.url, Some(&fx.token), Some("foreign-session-from-a")).await,
        StatusCode::OK
    );
}

#[test]
fn parse_bearer_rejects_non_bearer() {
    assert!(parse_bearer_token(None).is_none());
    assert!(parse_bearer_token(Some("Basic x")).is_none());
    assert_eq!(parse_bearer_token(Some("Bearer tok")), Some("tok"));
}
