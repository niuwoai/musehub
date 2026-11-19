use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::auth::{hash_token, session_cookie_with_secure, session_token};
use crate::error::AppError;
use crate::oidc::{append_cookie, cookie_value, safe_return_path};
use crate::AppState;

const BINDING_COOKIE: &str = "musehub_github_oauth_binding";
const ATTEMPT_TTL_MINUTES: i64 = 10;
const SESSION_TTL_DAYS: i64 = 14;
const GITHUB_ISSUER: &str = "https://github.com";
const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";

#[derive(Clone)]
pub struct GithubOAuthProvider {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    token_url: String,
    user_url: String,
    secure_cookies: bool,
    http_client: reqwest::Client,
}

impl GithubOAuthProvider {
    pub fn new(
        config: &musehub_core::GithubOAuthConfig,
        secure_cookies: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let parsed = Url::parse(config.redirect_uri())?;
        let secure_redirect = parsed.scheme() == "https";
        let loopback_http = parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(secure_redirect || loopback_http)
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("GitHub OAuth redirect URI must use HTTPS (HTTP only on loopback) and contain no credentials or fragment".into());
        }
        Ok(Self {
            client_id: config.client_id().to_string(),
            client_secret: config.client_secret().to_string(),
            redirect_uri: config.redirect_uri().to_string(),
            token_url: GITHUB_TOKEN_URL.to_string(),
            user_url: GITHUB_USER_URL.to_string(),
            secure_cookies: secure_cookies || secure_redirect,
            http_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .user_agent("MuseHub")
                .build()?,
        })
    }
}

#[derive(Deserialize)]
pub struct StartQuery {
    return_to: Option<String>,
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    id: u64,
    login: String,
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    client_id: &'a str,
    client_secret: &'a str,
    code: &'a str,
    redirect_uri: &'a str,
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StartQuery>,
) -> Result<Response, AppError> {
    let provider = state
        .github_oauth_provider
        .as_ref()
        .ok_or(AppError::NotFound)?;
    let mut authorize_url = Url::parse(GITHUB_AUTHORIZE_URL)
        .map_err(|_| AppError::Other("invalid GitHub authorization endpoint".into()))?;
    let csrf_state = session_token();
    let binding = session_token();
    authorize_url
        .query_pairs_mut()
        .append_pair("client_id", &provider.client_id)
        .append_pair("redirect_uri", &provider.redirect_uri)
        .append_pair("scope", "read:user")
        .append_pair("state", &csrf_state);

    let return_path = safe_return_path(query.return_to.as_deref());
    let expires_at = OffsetDateTime::now_utc() + time::Duration::minutes(ATTEMPT_TTL_MINUTES);
    sqlx::query("DELETE FROM github_login_attempts WHERE expires_at <= CURRENT_TIMESTAMP")
        .execute(&state.pool)
        .await?;
    sqlx::query("INSERT INTO github_login_attempts (state_hash, binding_hash, return_path, expires_at) VALUES ($1,$2,$3,$4)")
        .bind(hash_token(&csrf_state)).bind(hash_token(&binding)).bind(return_path).bind(expires_at)
        .execute(&state.pool).await?;

    let mut response = Redirect::to(authorize_url.as_str()).into_response();
    let cookie = axum_extra::extract::cookie::Cookie::build((BINDING_COOKIE, binding))
        .path("/api/v1/auth/github/callback")
        .http_only(true)
        .secure(provider.secure_cookies)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .max_age(time::Duration::minutes(ATTEMPT_TTL_MINUTES))
        .build();
    append_cookie(&mut response, cookie.to_string())?;
    Ok(response)
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let provider = state
        .github_oauth_provider
        .as_ref()
        .ok_or(AppError::NotFound)?;
    if query.error.is_some() {
        consume_attempt(&state.pool, query.state.as_deref(), &headers).await?;
        return Err(AppError::Unauthorized);
    }
    let code = query
        .code
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let return_path = consume_attempt(&state.pool, query.state.as_deref(), &headers).await?;
    let body = serde_urlencoded::to_string(TokenRequest {
        client_id: &provider.client_id,
        client_secret: &provider.client_secret,
        code,
        redirect_uri: &provider.redirect_uri,
    })
    .map_err(|_| AppError::Unauthorized)?;
    let token_response = provider
        .http_client
        .post(&provider.token_url)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|_| AppError::Unauthorized)?
        .error_for_status()
        .map_err(|_| AppError::Unauthorized)?
        .text()
        .await
        .map_err(|_| AppError::Unauthorized)?;
    let token_response: TokenResponse =
        serde_json::from_str(&token_response).map_err(|_| AppError::Unauthorized)?;
    if token_response.error.is_some() {
        return Err(AppError::Unauthorized);
    }
    let access_token = token_response
        .access_token
        .filter(|value| !value.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let github_response = provider
        .http_client
        .get(&provider.user_url)
        .header(header::ACCEPT, "application/vnd.github+json")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| AppError::Unauthorized)?
        .error_for_status()
        .map_err(|_| AppError::Unauthorized)?
        .text()
        .await
        .map_err(|_| AppError::Unauthorized)?;
    let github_user: GithubUser =
        serde_json::from_str(&github_response).map_err(|_| AppError::Unauthorized)?;
    if github_user.id == 0 || github_user.login.trim().is_empty() {
        return Err(AppError::Unauthorized);
    }
    let principal_id = establish_github_principal(
        &state.pool,
        github_user.id,
        &github_user.login,
        crate::audit::request_id_from_headers(&headers),
    )
    .await?;
    let token = session_token();
    let expires_at = OffsetDateTime::now_utc() + time::Duration::days(SESSION_TTL_DAYS);
    sqlx::query(
        "INSERT INTO sessions (id, principal_id, token_hash, expires_at) VALUES ($1,$2,$3,$4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal_id)
    .bind(hash_token(&token))
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    let mut response = Redirect::to(&return_path).into_response();
    append_cookie(
        &mut response,
        session_cookie_with_secure(&token, provider.secure_cookies).to_string(),
    )?;
    let clear_binding = axum_extra::extract::cookie::Cookie::build((BINDING_COOKIE, ""))
        .path("/api/v1/auth/github/callback")
        .http_only(true)
        .secure(provider.secure_cookies)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .max_age(time::Duration::seconds(0))
        .build();
    append_cookie(&mut response, clear_binding.to_string())?;
    Ok(response)
}

async fn consume_attempt(
    pool: &PgPool,
    raw_state: Option<&str>,
    headers: &HeaderMap,
) -> Result<String, AppError> {
    let raw_state = raw_state
        .filter(|value| !value.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let binding = cookie_value(headers, BINDING_COOKIE).ok_or(AppError::Unauthorized)?;
    sqlx::query_scalar::<_, String>(
        "DELETE FROM github_login_attempts WHERE state_hash=$1 AND binding_hash=$2 AND expires_at>CURRENT_TIMESTAMP RETURNING return_path",
    )
    .bind(hash_token(raw_state)).bind(hash_token(binding)).fetch_optional(pool).await?.ok_or(AppError::Unauthorized)
}

async fn establish_github_principal(
    pool: &PgPool,
    github_id: u64,
    github_login: &str,
    request_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    let subject = github_id.to_string();
    let mut tx = pool.begin().await?;
    let existing = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT p.id, p.status FROM external_identities i JOIN principals p ON p.id=i.principal_id WHERE i.issuer=$1 AND i.subject=$2 AND p.kind='human' FOR UPDATE OF p",
    )
    .bind(GITHUB_ISSUER).bind(&subject).fetch_optional(&mut *tx).await?;
    let (principal_id, created) = match existing {
        Some((id, status)) if status == "active" => (id, false),
        Some(_) => return Err(AppError::Forbidden),
        None => {
            let id = Uuid::new_v4();
            let handle = format!("github-{github_id}");
            sqlx::query("INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status) VALUES ($1,'human',$2,$2,$3,'active')")
                .bind(id).bind(handle).bind(github_login).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO external_identities (id, principal_id, issuer, subject) VALUES ($1,$2,$3,$4)")
                .bind(Uuid::new_v4()).bind(id).bind(GITHUB_ISSUER).bind(&subject).execute(&mut *tx).await?;
            (id, true)
        }
    };
    if created {
        crate::audit::record_in_tx(
            &mut tx,
            None,
            None,
            principal_id,
            None,
            "auth.github_provision",
            "principal",
            principal_id,
            request_id,
            "allow",
            serde_json::json!({ "provider": "github" }),
        )
        .await?;
    }
    crate::audit::record_in_tx(
        &mut tx,
        None,
        None,
        principal_id,
        None,
        "auth.github_login",
        "principal",
        principal_id,
        request_id,
        "allow",
        serde_json::json!({ "provider": "github" }),
    )
    .await?;
    tx.commit().await?;
    Ok(principal_id)
}

#[cfg(test)]
mod tests {
    use super::{
        callback, consume_attempt, establish_github_principal, CallbackQuery, GithubOAuthProvider,
        BINDING_COOKIE,
    };
    use crate::auth::{hash_token, session_token};
    use crate::error::AppError;
    use axum::extract::{Query, State};
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
    use musehub_migrate::{load_migrations, migrate_up};
    use sqlx::{postgres::PgPoolOptions, PgPool};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;
    use time::OffsetDateTime;
    use uuid::Uuid;

    async fn test_pool() -> PgPool {
        let database_url = std::env::var("MUSEHUB_TEST_DATABASE_URL")
            .expect("MUSEHUB_TEST_DATABASE_URL must point to a disposable test database");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("connect to test database");
        let migrations =
            load_migrations(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations"))
                .expect("load migrations");
        migrate_up(&pool, &migrations)
            .await
            .expect("migrate test database");
        pool
    }

    #[tokio::test]
    async fn github_login_attempt_requires_matching_state_and_browser_binding_once() {
        let pool = test_pool().await;
        let state = session_token();
        let binding = session_token();
        let return_path = "/settings/security";
        let expires_at = OffsetDateTime::now_utc() + time::Duration::minutes(10);
        sqlx::query(
            "INSERT INTO github_login_attempts (state_hash, binding_hash, return_path, expires_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(hash_token(&state))
        .bind(hash_token(&binding))
        .bind(return_path)
        .bind(expires_at)
        .execute(&pool)
        .await
        .expect("insert login attempt");

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{BINDING_COOKIE}={binding}"))
                .expect("valid binding cookie"),
        );
        assert!(matches!(
            consume_attempt(&pool, Some("wrong-state"), &headers).await,
            Err(AppError::Unauthorized)
        ));
        assert_eq!(
            consume_attempt(&pool, Some(&state), &headers)
                .await
                .expect("matching attempt"),
            return_path
        );
        assert!(matches!(
            consume_attempt(&pool, Some(&state), &headers).await,
            Err(AppError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn github_principal_uses_stable_id_and_does_not_link_by_login() {
        let pool = test_pool().await;
        let first_id = Uuid::new_v4().as_u128() as u64;
        let second_id = first_id.wrapping_add(1);
        let login = format!("test-login-{}", Uuid::new_v4().simple());

        let first = establish_github_principal(&pool, first_id, &login, None)
            .await
            .expect("provision first GitHub identity");
        let first_again = establish_github_principal(&pool, first_id, &login, None)
            .await
            .expect("reuse existing GitHub identity");
        let second = establish_github_principal(&pool, second_id, &login, None)
            .await
            .expect("provision second GitHub identity with same login");

        assert_eq!(first, first_again);
        assert_ne!(first, second);
        let account_state = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT
               (SELECT COUNT(*) FROM user_credentials WHERE principal_id IN ($1, $2)),
               (SELECT COUNT(*) FROM memberships WHERE principal_id IN ($1, $2)),
               (SELECT COUNT(*) FROM external_identities
                WHERE issuer = $3 AND subject IN ($4, $5))",
        )
        .bind(first)
        .bind(second)
        .bind(super::GITHUB_ISSUER)
        .bind(first_id.to_string())
        .bind(second_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("query provisioned accounts");
        assert_eq!(account_state, (0, 0, 2));
    }

    #[tokio::test]
    async fn github_callback_exchanges_code_and_sets_session_with_mock_provider() {
        let pool = test_pool().await;
        let state = session_token();
        let binding = session_token();
        sqlx::query(
            "INSERT INTO github_login_attempts (state_hash, binding_hash, return_path, expires_at)
             VALUES ($1, $2, '/org/demo/repos', $3)",
        )
        .bind(hash_token(&state))
        .bind(hash_token(&binding))
        .bind(OffsetDateTime::now_utc() + time::Duration::minutes(10))
        .execute(&pool)
        .await
        .expect("insert login attempt");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
        let address = listener.local_addr().expect("mock provider address");
        let mock_provider = thread::spawn(move || {
            for (expected_path, body) in [
                ("/token", r#"{"access_token":"mock-access-token"}"#),
                ("/user", r#"{"id":918273645,"login":"mock-user"}"#),
            ] {
                let (mut stream, _) = listener.accept().expect("accept provider request");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("set provider timeout");
                let mut request = Vec::new();
                let mut chunk = [0_u8; 2048];
                loop {
                    let count = stream.read(&mut chunk).expect("read provider request");
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..count]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(header_end) = text.find("\r\n\r\n") {
                        let content_length = text[..header_end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if request.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                let request_line = String::from_utf8_lossy(&request);
                let actual_path = request_line
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .expect("provider request path");
                assert_eq!(actual_path, expected_path);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write provider response");
            }
        });

        let provider = GithubOAuthProvider {
            client_id: "mock-client".into(),
            client_secret: "mock-secret".into(),
            redirect_uri: "http://127.0.0.1:8080/api/v1/auth/github/callback".into(),
            token_url: format!("http://{address}/token"),
            user_url: format!("http://{address}/user"),
            secure_cookies: false,
            http_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("mock OAuth HTTP client"),
        };
        let temp_root = tempfile::tempdir().expect("temporary app root");
        let app_state = Arc::new(crate::AppState {
            pool: pool.clone(),
            git_root: temp_root.path().to_path_buf(),
            artifact_root: temp_root.path().join("artifacts"),
            bind_display: "127.0.0.1".into(),
            reference_adapter_bin: PathBuf::new(),
            pipeline_adapter_bin: PathBuf::new(),
            compose_adapter_bin: PathBuf::new(),
            compose_runner_bin: PathBuf::new(),
            instance_id: crate::lease::parse_instance_id(),
            supervisor: crate::agent_supervisor::new_supervisor(),
            pipeline_supervisor: crate::pipeline_supervisor::PipelineSupervisor::new(),
            require_pipeline_oci: false,
            mcp_pat_slot: Arc::new(tokio::sync::RwLock::new(None)),
            mcp_request_gate: Arc::new(tokio::sync::Mutex::new(())),
            oidc_provider: None,
            saml_provider: None,
            github_oauth_provider: Some(Arc::new(provider)),
            cookie_secure: false,
        });
        let sessions_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions s
             JOIN external_identities i ON i.principal_id = s.principal_id
             WHERE i.issuer = $1 AND i.subject = '918273645'",
        )
        .bind(super::GITHUB_ISSUER)
        .fetch_one(&pool)
        .await
        .expect("query sessions before callback");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{BINDING_COOKIE}={binding}"))
                .expect("valid binding cookie"),
        );
        let response = callback(
            State(app_state),
            headers,
            Query(CallbackQuery {
                code: Some("mock-code".into()),
                state: Some(state),
                error: None,
            }),
        )
        .await
        .expect("complete mocked GitHub callback");
        mock_provider.join().expect("mock provider requests");

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookies = response.headers().get_all(header::SET_COOKIE);
        assert!(cookies.iter().any(|cookie| {
            cookie
                .to_str()
                .is_ok_and(|value| value.starts_with("musehub_session="))
        }));
        let sessions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions s
             JOIN external_identities i ON i.principal_id = s.principal_id
             WHERE i.issuer = $1 AND i.subject = '918273645'",
        )
        .bind(super::GITHUB_ISSUER)
        .fetch_one(&pool)
        .await
        .expect("query callback session");
        assert_eq!(sessions, sessions_before + 1);
    }
}
