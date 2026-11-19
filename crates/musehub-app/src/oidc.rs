use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::reqwest;
use openidconnect::{
    Audience, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse,
};
use serde::Deserialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::{cookie_name, hash_token, session_cookie_with_secure, session_token};
use crate::error::AppError;
use crate::AppState;

const BINDING_COOKIE: &str = "musehub_oidc_binding";
const ATTEMPT_TTL_MINUTES: i64 = 10;
const SESSION_TTL_DAYS: i64 = 14;
type OidcCoreClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

#[derive(Clone)]
pub struct OidcProvider {
    client: OidcCoreClient,
    http_client: reqwest::Client,
    issuer: String,
    secure_cookies: bool,
}

impl OidcProvider {
    pub async fn discover(
        issuer: &str,
        client_id: &str,
        client_secret: Option<&str>,
        redirect_uri: &str,
        secure_cookies: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let issuer_url = IssuerUrl::new(issuer.to_string())?;
        let redirect = RedirectUrl::new(redirect_uri.to_string())?;
        let parsed_redirect = url::Url::parse(redirect_uri)?;
        let redirect_is_secure = parsed_redirect.scheme() == "https";
        if !(redirect_is_secure
            || parsed_redirect.scheme() == "http"
                && matches!(
                    parsed_redirect.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]")
                ))
        {
            return Err(
                "OIDC redirect URI must use HTTPS (HTTP is allowed only on loopback)".into(),
            );
        }
        if parsed_redirect.fragment().is_some()
            || parsed_redirect.username() != ""
            || parsed_redirect.password().is_some()
        {
            return Err("OIDC redirect URI must not contain credentials or a fragment".into());
        }
        let secure_cookies = secure_cookies || redirect_is_secure;

        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        let metadata = CoreProviderMetadata::discover_async(issuer_url, &http_client).await?;
        let provider_issuer = metadata.issuer().to_string();
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(client_id.to_string()),
            client_secret.map(|value| ClientSecret::new(value.to_string())),
        )
        .set_redirect_uri(redirect);

        Ok(Self {
            client,
            http_client,
            issuer: provider_issuer,
            secure_cookies,
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

pub async fn providers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "oidc": state.oidc_provider.is_some(),
        "github": state.github_oauth_provider.is_some(),
        "saml": state.saml_provider.is_some()
    }))
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StartQuery>,
) -> Result<Response, AppError> {
    let provider = state.oidc_provider.as_ref().ok_or(AppError::NotFound)?;
    begin_authorization(
        &state,
        provider,
        safe_return_path(query.return_to.as_deref()),
        None,
        None,
    )
    .await
}

pub async fn start_link(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<StartQuery>,
) -> Result<Response, AppError> {
    let principal_id = crate::require_session(&state.pool, &headers).await?;
    let session_token = cookie_value(&headers, cookie_name()).ok_or(AppError::Unauthorized)?;
    let provider = state.oidc_provider.as_ref().ok_or(AppError::NotFound)?;
    begin_authorization(
        &state,
        provider,
        safe_return_path(query.return_to.as_deref()),
        Some(principal_id),
        Some(hash_token(session_token)),
    )
    .await
}

pub async fn identity_link_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let principal_id = crate::require_session(&state.pool, &headers).await?;
    let Some(provider) = state.oidc_provider.as_ref() else {
        return Ok(Json(
            serde_json::json!({ "enabled": false, "linked": false }),
        ));
    };
    let linked = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM external_identities
            WHERE principal_id = $1 AND issuer = $2
        )",
    )
    .bind(principal_id)
    .bind(&provider.issuer)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(
        serde_json::json!({ "enabled": true, "linked": linked }),
    ))
}

async fn begin_authorization(
    state: &AppState,
    provider: &OidcProvider,
    return_to: String,
    link_principal_id: Option<Uuid>,
    link_session_token_hash: Option<Vec<u8>>,
) -> Result<Response, AppError> {
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorize_url, csrf_state, nonce) = provider
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .set_pkce_challenge(challenge)
        .url();
    let binding = session_token();
    let state_hash = hash_token(csrf_state.secret());
    let binding_hash = hash_token(&binding);
    let expires_at = OffsetDateTime::now_utc() + time::Duration::minutes(ATTEMPT_TTL_MINUTES);
    sqlx::query("DELETE FROM oidc_login_attempts WHERE expires_at <= CURRENT_TIMESTAMP")
        .execute(&state.pool)
        .await?;
    sqlx::query(
        "INSERT INTO oidc_login_attempts
         (state_hash, binding_hash, nonce, pkce_verifier, return_path,
          link_principal_id, link_session_token_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(state_hash)
    .bind(binding_hash)
    .bind(nonce.secret())
    .bind(verifier.secret())
    .bind(return_to)
    .bind(link_principal_id)
    .bind(link_session_token_hash)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    let mut response = Redirect::to(authorize_url.as_str()).into_response();
    let cookie = axum_extra::extract::cookie::Cookie::build((BINDING_COOKIE, binding))
        .path("/api/v1/auth/oidc/callback")
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
    let provider = state.oidc_provider.as_ref().ok_or(AppError::NotFound)?;
    if query.error.is_some() {
        consume_attempt(&state.pool, query.state.as_deref(), &headers).await?;
        return Err(AppError::Unauthorized);
    }
    let code = query.code.as_deref().ok_or(AppError::Unauthorized)?;
    let (nonce, verifier, return_path, link_principal_id, link_session_token_hash) =
        consume_attempt(&state.pool, query.state.as_deref(), &headers).await?;
    let token_response = provider
        .client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .map_err(|_| AppError::Unauthorized)?
        .set_pkce_verifier(PkceCodeVerifier::new(verifier))
        .request_async(&provider.http_client)
        .await
        .map_err(|_| AppError::Unauthorized)?;
    let id_token = token_response.id_token().ok_or(AppError::Unauthorized)?;
    let claims = id_token
        .claims(&provider.client.id_token_verifier(), &Nonce::new(nonce))
        .map_err(|_| AppError::Unauthorized)?;
    let audiences = claims.audiences();
    let client_id = provider.client.client_id();
    let client_audience = Audience::new(client_id.to_string());
    if !audiences
        .iter()
        .any(|audience| audience == &client_audience)
        || (audiences.len() > 1 && claims.authorized_party() != Some(client_id))
        || claims
            .authorized_party()
            .is_some_and(|authorized_party| authorized_party != client_id)
    {
        return Err(AppError::Unauthorized);
    }
    let issuer = claims.issuer().as_str();
    if issuer != provider.issuer {
        return Err(AppError::Unauthorized);
    }
    let subject = claims.subject().as_str();
    let mut response = Redirect::to(&return_path).into_response();
    if let Some(principal_id) = link_principal_id {
        link_external_identity(
            &state.pool,
            principal_id,
            link_session_token_hash
                .as_deref()
                .ok_or(AppError::Unauthorized)?,
            issuer,
            subject,
        )
        .await?;
    } else {
        let principal_id = lookup_active_principal(&state.pool, issuer, subject).await?;
        let token = session_token();
        let expires_at = OffsetDateTime::now_utc() + time::Duration::days(SESSION_TTL_DAYS);
        sqlx::query(
            "INSERT INTO sessions (id, principal_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(principal_id)
        .bind(hash_token(&token))
        .bind(expires_at)
        .execute(&state.pool)
        .await?;
        let cookie = session_cookie_with_secure(&token, provider.secure_cookies);
        append_cookie(&mut response, cookie.to_string())?;
        if let Err(error) = audit_oidc_login(
            &state.pool,
            principal_id,
            issuer,
            crate::audit::request_id_from_headers(&headers),
        )
        .await
        {
            tracing::warn!(principal_id = %principal_id, %error, "failed to record OIDC login audit event");
        }
    }
    let clear_binding = axum_extra::extract::cookie::Cookie::build((BINDING_COOKIE, ""))
        .path("/api/v1/auth/oidc/callback")
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
) -> Result<(String, String, String, Option<Uuid>, Option<Vec<u8>>), AppError> {
    let raw_state = raw_state
        .filter(|value| !value.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let binding = cookie_value(headers, BINDING_COOKIE).ok_or(AppError::Unauthorized)?;
    let row = sqlx::query_as::<_, (String, String, String, Option<Uuid>, Option<Vec<u8>>)>(
        "DELETE FROM oidc_login_attempts
         WHERE state_hash = $1 AND binding_hash = $2 AND expires_at > CURRENT_TIMESTAMP
         RETURNING nonce, pkce_verifier, return_path, link_principal_id, link_session_token_hash",
    )
    .bind(hash_token(raw_state))
    .bind(hash_token(binding))
    .fetch_optional(pool)
    .await?;
    row.ok_or(AppError::Unauthorized)
}

async fn link_external_identity(
    pool: &PgPool,
    principal_id: Uuid,
    session_token_hash: &[u8],
    issuer: &str,
    subject: &str,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let active_session = sqlx::query_scalar::<_, Uuid>(
        "SELECT s.id FROM sessions s
         JOIN principals p ON p.id = s.principal_id
         WHERE s.token_hash = $1 AND s.principal_id = $2
           AND s.expires_at > CURRENT_TIMESTAMP AND s.revoked_at IS NULL
           AND p.kind = 'human' AND p.status = 'active'
         FOR UPDATE OF s",
    )
    .bind(session_token_hash)
    .bind(principal_id)
    .fetch_optional(&mut *tx)
    .await?;
    if active_session.is_none() {
        return Err(AppError::Unauthorized);
    }

    let identity_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO external_identities (id, principal_id, issuer, subject)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (issuer, subject) DO NOTHING
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(principal_id)
    .bind(issuer)
    .bind(subject)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(identity_id) = identity_id else {
        let existing_principal_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT principal_id FROM external_identities
             WHERE issuer = $1 AND subject = $2",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(existing_principal_id) = existing_principal_id else {
            return Err(AppError::Conflict(
                "external identity changed during linking; retry".to_string(),
            ));
        };
        if existing_principal_id != principal_id {
            return Err(AppError::Conflict(
                "external identity is already linked to another account".to_string(),
            ));
        }
        tx.commit().await?;
        return Ok(());
    };

    crate::audit::record_in_tx(
        &mut tx,
        None,
        None,
        principal_id,
        None,
        "auth.oidc_identity_link",
        "external_identity",
        identity_id,
        None,
        "allow",
        serde_json::json!({ "issuer": issuer }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn lookup_active_principal(
    pool: &PgPool,
    issuer: &str,
    subject: &str,
) -> Result<Uuid, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT p.id, p.status
         FROM external_identities i
         JOIN principals p ON p.id = i.principal_id
         WHERE i.issuer = $1 AND i.subject = $2 AND p.kind = 'human'",
    )
    .bind(issuer)
    .bind(subject)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((principal_id, status)) if status == "active" => Ok(principal_id),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::Forbidden),
    }
}

async fn audit_oidc_login(
    pool: &PgPool,
    principal_id: Uuid,
    issuer: &str,
    request_id: Option<Uuid>,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    crate::audit::record_in_tx(
        &mut tx,
        None,
        None,
        principal_id,
        None,
        "auth.oidc_login",
        "principal",
        principal_id,
        request_id,
        "allow",
        serde_json::json!({ "issuer": issuer }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

pub(crate) fn append_cookie(response: &mut Response, value: String) -> Result<(), AppError> {
    let value = HeaderValue::from_str(&value)
        .map_err(|_| AppError::Other("invalid cookie header".into()))?;
    response.headers_mut().append(header::SET_COOKIE, value);
    Ok(())
}

pub(crate) fn safe_return_path(value: Option<&str>) -> String {
    match value {
        Some(path)
            if path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.chars().any(char::is_control) =>
        {
            path.to_string()
        }
        _ => "/".to_string(),
    }
}
