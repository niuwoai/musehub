use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    extract::{Form, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Redirect, Response},
};
use saml_rs::{
    AcsEndpoint, BrowserInput, CertificatePem, EndpointUrl, EntityId, FormField, IdpDescriptor,
    MetadataTrustPolicy, PendingAuthnRequest, PendingSnapshot, RelayStateParam, ReplayCache,
    ReplayKey, ReplayPolicy, Saml, SamlError, SamlValidationContext, Sp, SpConfig, SsoResponse,
    SsoResponseBinding, StartSso,
};
use serde::Deserialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    auth::{hash_token, session_cookie_with_secure, session_token},
    error::AppError,
    oidc::{append_cookie, cookie_value, safe_return_path},
    AppState,
};

const BINDING_COOKIE: &str = "musehub_saml_binding";
const ATTEMPT_TTL_MINUTES: i64 = 10;
const REPLAY_TTL_MINUTES: i64 = 10;
const SESSION_TTL_DAYS: i64 = 14;

pub struct SamlProvider {
    sp: Saml<Sp>,
    idp: IdpDescriptor,
    entity_id: String,
    acs_url: String,
    secure_cookies: bool,
}

impl SamlProvider {
    pub fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error + Send + Sync>> {
        let names = [
            "MUSEHUB_SAML_IDP_ENTITY_ID",
            "MUSEHUB_SAML_IDP_METADATA_PATH",
            "MUSEHUB_SAML_METADATA_SIGNING_CERT_PATH",
            "MUSEHUB_SAML_SP_ENTITY_ID",
            "MUSEHUB_SAML_ACS_URL",
        ];
        let values = names.map(std::env::var_os);
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        if values.iter().any(Option::is_none) {
            return Err("SAML requires all five MUSEHUB_SAML_* configuration values".into());
        }
        let values: Vec<String> = values
            .into_iter()
            .map(|value| {
                value
                    .expect("checked all SAML configuration values are present")
                    .into_string()
                    .map_err(|_| "SAML configuration values must be valid Unicode")
            })
            .collect::<Result<_, _>>()?;
        let (idp_entity, metadata_path, cert_path, sp_entity, acs_url) =
            (&values[0], &values[1], &values[2], &values[3], &values[4]);
        let acs = url::Url::parse(acs_url)?;
        if acs.scheme() != "https"
            || acs.fragment().is_some()
            || !acs.username().is_empty()
            || acs.password().is_some()
        {
            return Err("SAML ACS URL must be HTTPS and contain no credentials or fragment".into());
        }
        let metadata = std::fs::read_to_string(metadata_path)?;
        let cert = CertificatePem::new(std::fs::read_to_string(cert_path)?);
        let idp = IdpDescriptor::from_metadata_xml_for(
            EntityId::try_new(idp_entity.clone())?,
            &metadata,
            MetadataTrustPolicy::RequireSignature {
                trusted_certificates: std::slice::from_ref(&cert),
            },
        )?;
        let config = SpConfig::builder(EntityId::try_new(sp_entity.clone())?)
            .acs_endpoint(AcsEndpoint::post(acs_url.clone())?)
            .build()?;
        Ok(Some(Self {
            sp: Saml::sp(config)?,
            idp,
            entity_id: idp_entity.clone(),
            acs_url: acs_url.clone(),
            secure_cookies: true,
        }))
    }
}

#[derive(Deserialize)]
pub struct StartQuery {
    return_to: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SamlAcsForm {
    #[serde(rename = "SAMLResponse")]
    saml_response: String,
    #[serde(rename = "RelayState")]
    relay_state: String,
}

pub async fn metadata(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let provider = state.saml_provider.as_ref().ok_or(AppError::NotFound)?;
    let response = Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/samlmetadata+xml; charset=utf-8",
        )
        .body(axum::body::Body::from(
            provider.sp.metadata_xml().to_owned(),
        ))
        .map_err(|error| AppError::Other(error.to_string()))?;
    Ok(response)
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<StartQuery>,
) -> Result<Response, AppError> {
    let provider = state.saml_provider.as_ref().ok_or(AppError::NotFound)?;
    let return_path = safe_return_path(query.return_to.as_deref());
    let relay = session_token();
    let binding = session_token();
    let relay_param = RelayStateParam::try_from_option(Some(relay.clone()))
        .map_err(|_| AppError::Unauthorized)?;
    let started = provider
        .sp
        .start_sso(
            &provider.idp,
            StartSso::redirect()
                .response_binding(SsoResponseBinding::Post)
                .relay_state(relay_param),
        )
        .map_err(|_| AppError::Unauthorized)?;
    let snapshot = started.pending.snapshot();
    let expires_at = OffsetDateTime::now_utc() + time::Duration::minutes(ATTEMPT_TTL_MINUTES);
    sqlx::query("DELETE FROM saml_login_attempts WHERE expires_at <= CURRENT_TIMESTAMP")
        .execute(&state.pool)
        .await?;
    sqlx::query("DELETE FROM saml_replay_entries WHERE expires_at <= CURRENT_TIMESTAMP")
        .execute(&state.pool)
        .await?;
    sqlx::query("INSERT INTO saml_login_attempts (relay_hash, binding_hash, request_id, idp_entity_id, expected_binding, acs_url, acs_binding, return_path, expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(hash_token(&relay)).bind(hash_token(&binding)).bind(snapshot.id).bind(snapshot.peer_entity_id).bind(snapshot.expected_binding).bind(snapshot.acs_url).bind(snapshot.acs_binding).bind(return_path).bind(expires_at).execute(&state.pool).await?;
    let redirect = started
        .outbound
        .redirect_url()
        .map_err(|_| AppError::Unauthorized)?;
    let mut response = Redirect::to(redirect).into_response();
    let cookie = axum_extra::extract::cookie::Cookie::build((BINDING_COOKIE, binding))
        .path("/api/v1/auth/saml/acs")
        .http_only(true)
        .secure(true)
        .same_site(axum_extra::extract::cookie::SameSite::None)
        .max_age(time::Duration::minutes(ATTEMPT_TTL_MINUTES))
        .build();
    append_cookie(&mut response, cookie.to_string())?;
    Ok(response)
}

pub(crate) async fn acs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(fields): Form<SamlAcsForm>,
) -> Result<Response, AppError> {
    let provider = state.saml_provider.as_ref().ok_or(AppError::NotFound)?;
    let relay = &fields.relay_state;
    let response_xml = &fields.saml_response;
    if relay.is_empty() || response_xml.is_empty() {
        return Err(AppError::Unauthorized);
    }
    let binding = cookie_value(&headers, BINDING_COOKIE).ok_or(AppError::Unauthorized)?;
    let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
        "DELETE FROM saml_login_attempts WHERE relay_hash=$1 AND binding_hash=$2 AND expires_at>CURRENT_TIMESTAMP RETURNING request_id,idp_entity_id,expected_binding,acs_url,acs_binding,return_path")
        .bind(hash_token(relay)).bind(hash_token(binding)).fetch_optional(&state.pool).await?.ok_or(AppError::Unauthorized)?;
    let (request_id, idp_entity, expected_binding, acs_url, acs_binding, return_path) = row;
    if idp_entity != provider.entity_id
        || acs_url != provider.acs_url
        || expected_binding != "post"
        || acs_binding != "post"
    {
        return Err(AppError::Unauthorized);
    }
    let snapshot = PendingSnapshot::<saml_rs::AuthnRequest>::authn_request(
        request_id,
        RelayStateParam::try_from_option(Some(relay.clone()))
            .map_err(|_| AppError::Unauthorized)?,
        idp_entity,
        "post",
        acs_url.clone(),
        "post",
    );
    let pending =
        PendingAuthnRequest::from_snapshot(snapshot).map_err(|_| AppError::Unauthorized)?;
    let mut saml_fields = vec![
        FormField::new("SAMLResponse", response_xml.clone()),
        FormField::new("RelayState", relay.clone()),
    ];
    saml_fields.shrink_to_fit();
    let input = BrowserInput::<SsoResponse>::post(saml_fields);
    let endpoint = EndpointUrl::try_new(acs_url).map_err(|_| AppError::Unauthorized)?;
    let mut replay = PgReplayCache {
        pool: state.pool.clone(),
    };
    let validation =
        SamlValidationContext::new(SystemTime::now(), ReplayPolicy::RequireCache(&mut replay))
            .with_recipient_endpoint(endpoint)
            .with_replay_retention(Duration::from_secs((REPLAY_TTL_MINUTES * 60) as u64));
    let saml_session = provider
        .sp
        .finish_sso(&provider.idp, &pending, input, validation)
        .map_err(|error| {
            tracing::warn!(%error, "SAML response validation failed");
            AppError::Unauthorized
        })?;
    let issuer = saml_session.issuer().as_str();
    if issuer != provider.entity_id {
        return Err(AppError::Unauthorized);
    }
    let subject = saml_session.name_id().value();
    let principal_id = lookup_active_principal(&state.pool, issuer, subject).await?;
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
    let cookie = session_cookie_with_secure(&token, provider.secure_cookies);
    append_cookie(&mut response, cookie.to_string())?;
    let clear = axum_extra::extract::cookie::Cookie::build((BINDING_COOKIE, ""))
        .path("/api/v1/auth/saml/acs")
        .http_only(true)
        .secure(true)
        .same_site(axum_extra::extract::cookie::SameSite::None)
        .max_age(time::Duration::seconds(0))
        .build();
    append_cookie(&mut response, clear.to_string())?;
    audit_saml_login(&state.pool, principal_id, issuer).await?;
    Ok(response)
}

struct PgReplayCache {
    pool: PgPool,
}
impl ReplayCache for PgReplayCache {
    fn check_and_store(&mut self, key: ReplayKey, expires_at: SystemTime) -> Result<(), SamlError> {
        let ttl = expires_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .as_secs()
            .min(i64::MAX as u64) as i64;
        if ttl <= 0 {
            return Ok(());
        }
        let pool = self.pool.clone();
        let cache_key = key.cache_key();
        let key_hash = hash_token(&cache_key);
        let insert_hash = key_hash.clone();
        let inserted = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async move {
            sqlx::query_scalar::<_, Vec<u8>>("INSERT INTO saml_replay_entries (cache_key_hash, expires_at) VALUES ($1, CURRENT_TIMESTAMP + ($2 * INTERVAL '1 second')) ON CONFLICT (cache_key_hash) DO UPDATE SET expires_at = EXCLUDED.expires_at WHERE saml_replay_entries.expires_at <= CURRENT_TIMESTAMP RETURNING cache_key_hash")
                .bind(&insert_hash).bind(ttl).fetch_optional(&pool).await
        })).map_err(|error| SamlError::Invalid(format!("durable SAML replay store failed: {error}")))?;
        if inserted.is_none() {
            return Err(SamlError::ReplayDetected { key: cache_key });
        }
        Ok(())
    }
}

async fn lookup_active_principal(
    pool: &PgPool,
    issuer: &str,
    subject: &str,
) -> Result<Uuid, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String)>("SELECT p.id,p.status FROM external_identities i JOIN principals p ON p.id=i.principal_id WHERE i.issuer=$1 AND i.subject=$2 AND p.kind='human'").bind(issuer).bind(subject).fetch_optional(pool).await?;
    match row {
        Some((id, status)) if status == "active" => Ok(id),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::Forbidden),
    }
}

async fn audit_saml_login(pool: &PgPool, principal_id: Uuid, issuer: &str) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    crate::audit::record_in_tx(
        &mut tx,
        None,
        None,
        principal_id,
        None,
        "auth.saml_login",
        "principal",
        principal_id,
        None,
        "allow",
        serde_json::json!({"issuer": issuer}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
