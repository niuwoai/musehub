//! Durable, tenant-scoped webhook delivery derived from committed audit events.
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use hmac::{Hmac, Mac};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

use crate::{error::AppError, AppState};

const MAX_EVENT_TYPES: usize = 16;
const MAX_ATTEMPTS: i32 = 8;
const BATCH_RETRY_SECONDS: u64 = 1;
const SUPPORTED_EVENTS: &[&str] = &[
    "org.member.invite",
    "issue.created",
    "pull_request.created",
    "pipeline.queued",
    "deployment.start",
    "deployment.approve",
    "deployment.reject",
    "deployment.rollback",
];

#[derive(Deserialize)]
pub struct CreateWebhookRequest {
    endpoint_url: String,
    event_types: Vec<String>,
    repo_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct WebhookView {
    id: Uuid,
    repo_id: Option<Uuid>,
    endpoint_url: String,
    event_types: Vec<String>,
    enabled: bool,
    created_at: String,
}

#[derive(Serialize)]
pub struct CreateWebhookResponse {
    subscription: WebhookView,
    secret: String,
}

#[derive(Serialize)]
pub struct DeliveryView {
    id: Uuid,
    subscription_id: Uuid,
    audit_event_id: Uuid,
    status: String,
    attempt_count: i32,
    response_status: Option<i32>,
    last_error: Option<String>,
    created_at: String,
}

pub async fn list_webhooks(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Json<Vec<WebhookView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = crate::members::require_org_admin(&state.pool, &org, principal_id).await?;
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            Option<Uuid>,
            String,
            Vec<String>,
            bool,
            time::OffsetDateTime,
        ),
    >(
        "SELECT id, repo_id, endpoint_url, event_types, enabled, created_at
         FROM webhook_subscriptions WHERE org_id = $1 ORDER BY created_at DESC, id DESC",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, repo_id, endpoint_url, event_types, enabled, created_at)| WebhookView {
                    id,
                    repo_id,
                    endpoint_url,
                    event_types,
                    enabled,
                    created_at: format_time(created_at),
                },
            )
            .collect(),
    ))
}

pub async fn create_webhook(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<CreateWebhookRequest>,
) -> Result<Json<CreateWebhookResponse>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = crate::members::require_org_admin(&state.pool, &org, principal_id).await?;
    let (endpoint_url, _) = validate_public_endpoint(&body.endpoint_url).await?;
    let mut event_types = body.event_types;
    event_types.sort();
    event_types.dedup();
    if event_types.is_empty()
        || event_types.len() > MAX_EVENT_TYPES
        || event_types
            .iter()
            .any(|event| !SUPPORTED_EVENTS.contains(&event.as_str()))
    {
        return Err(AppError::Validation(
            "event_types must contain supported event names".into(),
        ));
    }
    if let Some(repo_id) = body.repo_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM repositories WHERE id = $1 AND org_id = $2)",
        )
        .bind(repo_id)
        .bind(org_id)
        .fetch_one(&state.pool)
        .await?;
        if !exists {
            return Err(AppError::NotFound);
        }
    }
    let id = Uuid::new_v4();
    let mut raw_secret = [0_u8; 32];
    OsRng.fill_bytes(&mut raw_secret);
    let secret = URL_SAFE_NO_PAD.encode(raw_secret);
    let encrypted_secret = encrypt_secret(&raw_secret, id)?;
    let mut tx = state.pool.begin().await?;
    let created_at: time::OffsetDateTime = sqlx::query_scalar(
        "INSERT INTO webhook_subscriptions (id, org_id, repo_id, endpoint_url, encrypted_secret, event_types, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING created_at",
    )
    .bind(id).bind(org_id).bind(body.repo_id).bind(endpoint_url.as_str())
    .bind(encrypted_secret).bind(&event_types).bind(principal_id)
    .fetch_one(&mut *tx).await?;
    crate::audit::record_in_tx(
        &mut tx,
        Some(org_id),
        body.repo_id,
        principal_id,
        None,
        "webhook.subscription.create",
        "webhook_subscription",
        id,
        None,
        "allow",
        serde_json::json!({ "event_types": event_types, "repo_id": body.repo_id }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(CreateWebhookResponse {
        subscription: WebhookView {
            id,
            repo_id: body.repo_id,
            endpoint_url: endpoint_url.to_string(),
            event_types,
            enabled: true,
            created_at: format_time(created_at),
        },
        secret,
    }))
}

pub async fn disable_webhook(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Path((org, webhook_id)): Path<(String, Uuid)>,
) -> Result<axum::http::StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = crate::members::require_org_admin(&state.pool, &org, principal_id).await?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("UPDATE webhook_subscriptions SET enabled = FALSE, updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND org_id = $2 AND enabled RETURNING repo_id")
        .bind(webhook_id).bind(org_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        return Err(AppError::NotFound);
    };
    let repo_id: Option<Uuid> = sqlx::Row::try_get(&row, "repo_id")?;
    crate::audit::record_in_tx(
        &mut tx,
        Some(org_id),
        repo_id,
        principal_id,
        None,
        "webhook.subscription.disable",
        "webhook_subscription",
        webhook_id,
        None,
        "allow",
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn list_deliveries(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Path((org, webhook_id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<DeliveryView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = crate::members::require_org_admin(&state.pool, &org, principal_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, i32, Option<i32>, Option<String>, time::OffsetDateTime)>(
        "SELECT d.id, d.subscription_id, d.audit_event_id, d.status, d.attempt_count, d.response_status, d.last_error, d.created_at
         FROM webhook_deliveries d JOIN webhook_subscriptions s ON s.id = d.subscription_id
         WHERE s.org_id = $1 AND s.id = $2 ORDER BY d.created_at DESC, d.id DESC LIMIT 100",
    ).bind(org_id).bind(webhook_id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(
                    id,
                    subscription_id,
                    audit_event_id,
                    status,
                    attempt_count,
                    response_status,
                    last_error,
                    created_at,
                )| DeliveryView {
                    id,
                    subscription_id,
                    audit_event_id,
                    status,
                    attempt_count,
                    response_status,
                    last_error,
                    created_at: format_time(created_at),
                },
            )
            .collect(),
    ))
}

pub async fn run_webhook_delivery_worker(state: std::sync::Arc<AppState>) {
    loop {
        if let Err(error) = encryption_key() {
            tracing::error!(%error, "webhook delivery paused because encryption key configuration is invalid");
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        }
        match deliver_one(&state.pool).await {
            Ok(true) => {}
            Ok(false) => tokio::time::sleep(Duration::from_secs(BATCH_RETRY_SECONDS)).await,
            Err(error) => {
                tracing::error!(%error, "webhook delivery worker iteration failed");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

struct ClaimedDelivery {
    id: Uuid,
    subscription_id: Uuid,
    endpoint_url: String,
    encrypted_secret: Vec<u8>,
    event: serde_json::Value,
    attempt_count: i32,
}

async fn claim_delivery(pool: &PgPool) -> Result<Option<ClaimedDelivery>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Vec<u8>, Uuid, String, Option<Uuid>, Uuid, String, time::OffsetDateTime, i32)>(
        "SELECT d.id, s.id, e.id, s.endpoint_url, s.encrypted_secret, e.org_id, e.action, e.repo_id, e.resource_id, e.decision, e.occurred_at, d.attempt_count
         FROM webhook_deliveries d
         JOIN webhook_subscriptions s ON s.id = d.subscription_id AND s.enabled
         JOIN audit_events e ON e.id = d.audit_event_id
         WHERE ((d.status = 'pending' AND d.available_at <= CURRENT_TIMESTAMP)
             OR (d.status = 'delivering' AND d.locked_until < CURRENT_TIMESTAMP))
         ORDER BY d.available_at, d.created_at, d.id
         LIMIT 1 FOR UPDATE OF d SKIP LOCKED",
    ).fetch_optional(&mut *tx).await?;
    let Some((
        id,
        subscription_id,
        audit_event_id,
        endpoint_url,
        encrypted_secret,
        org_id,
        action,
        repo_id,
        resource_id,
        decision,
        occurred_at,
        attempt_count,
    )) = row
    else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("UPDATE webhook_deliveries SET status = 'delivering', attempt_count = attempt_count + 1, locked_until = CURRENT_TIMESTAMP + INTERVAL '30 seconds', updated_at = CURRENT_TIMESTAMP WHERE id = $1")
        .bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(ClaimedDelivery {
        id,
        subscription_id,
        endpoint_url,
        encrypted_secret,
        event: serde_json::json!({ "id": audit_event_id, "type": action, "organization_id": org_id, "repository_id": repo_id, "resource_id": resource_id, "decision": decision, "occurred_at": format_time(occurred_at) }),
        attempt_count: attempt_count + 1,
    }))
}

async fn deliver_one(pool: &PgPool) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let Some(delivery) = claim_delivery(pool).await? else {
        return Ok(false);
    };
    let result = send_delivery(&delivery).await;
    match result {
        Ok(status) if status.is_success() => {
            sqlx::query("UPDATE webhook_deliveries SET status = 'delivered', response_status = $2, locked_until = NULL, last_error = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = $1")
                .bind(delivery.id).bind(status.as_u16() as i32).execute(pool).await?;
        }
        Ok(status) => {
            retry_delivery(pool, &delivery, Some(status.as_u16() as i32), "http_status").await?
        }
        Err(_error) => {
            tracing::warn!(delivery_id = %delivery.id, "webhook delivery failed; retry scheduled");
            retry_delivery(pool, &delivery, None, "network_or_signing_error").await?;
        }
    }
    Ok(true)
}

async fn send_delivery(
    delivery: &ClaimedDelivery,
) -> Result<reqwest::StatusCode, Box<dyn std::error::Error + Send + Sync>> {
    let (url, addresses) = validate_public_endpoint(&delivery.endpoint_url).await?;
    let host = url.host_str().ok_or("webhook endpoint has no host")?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .resolve_to_addrs(host, &addresses)
        .build()?;
    send_delivery_with_client(delivery, url, &client).await
}

async fn send_delivery_with_client(
    delivery: &ClaimedDelivery,
    url: Url,
    client: &reqwest::Client,
) -> Result<reqwest::StatusCode, Box<dyn std::error::Error + Send + Sync>> {
    let secret = decrypt_secret(&delivery.encrypted_secret, delivery.subscription_id)?;
    let body = serde_json::to_vec(&delivery.event)?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret)?;
    mac.update(&body);
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("user-agent", "MuseHub-Webhook/1")
        .header(
            "x-musehub-event",
            delivery
                .event
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("audit"),
        )
        .header("x-musehub-delivery", delivery.id.to_string())
        .header("x-musehub-signature-256", format!("sha256={signature}"))
        .body(body)
        .send()
        .await?;
    Ok(response.status())
}

async fn retry_delivery(
    pool: &PgPool,
    delivery: &ClaimedDelivery,
    status: Option<i32>,
    error: &str,
) -> Result<(), sqlx::Error> {
    let (terminal, retry_seconds) = retry_schedule(delivery.attempt_count);
    sqlx::query(
        "UPDATE webhook_deliveries SET status = $2, response_status = $3, last_error = $4,
         available_at = CASE WHEN $2 = 'pending' THEN CURRENT_TIMESTAMP + ($5 * INTERVAL '1 second') ELSE available_at END,
         locked_until = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    ).bind(delivery.id).bind(if terminal { "failed" } else { "pending" }).bind(status).bind(error).bind(retry_seconds as f64)
        .execute(pool).await?;
    Ok(())
}

fn retry_schedule(attempt_count: i32) -> (bool, u64) {
    let terminal = attempt_count >= MAX_ATTEMPTS;
    let retry_seconds = 2_u64.saturating_pow(attempt_count.min(12) as u32).min(3600);
    (terminal, retry_seconds)
}

async fn validate_public_endpoint(raw: &str) -> Result<(Url, Vec<SocketAddr>), AppError> {
    let url = Url::parse(raw)
        .map_err(|_| AppError::Validation("endpoint_url must be a valid HTTPS URL".into()))?;
    let has_userinfo = url
        .as_str()
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .is_some_and(|authority| authority.contains('@'));
    if url.scheme() != "https"
        || url.host_str().is_none()
        || has_userinfo
        || url.fragment().is_some()
    {
        return Err(AppError::Validation(
            "endpoint_url must use HTTPS and cannot contain credentials or a fragment".into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppError::Validation("endpoint_url must include a hostname".into()))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| AppError::Validation("endpoint_url must include a valid port".into()))?;
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| AppError::Validation("endpoint hostname could not be resolved".into()))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(AppError::Validation(
            "endpoint hostname must resolve only to public IP addresses".into(),
        ));
    }
    Ok((url, addresses))
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            let in_cidr = |network: [u8; 4], prefix: u8| {
                let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
                (u32::from(ip) & mask) == (u32::from(std::net::Ipv4Addr::from(network)) & mask)
            };
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || octets[0] == 0
                || octets[0] >= 240
                || in_cidr([100, 64, 0, 0], 10)
                || in_cidr([192, 0, 0, 0], 24)
                || in_cidr([192, 0, 2, 0], 24)
                || in_cidr([192, 88, 99, 0], 24)
                || in_cidr([198, 18, 0, 0], 15)
                || in_cidr([198, 51, 100, 0], 24)
                || in_cidr([203, 0, 113, 0], 24))
        }
        IpAddr::V6(ip) => {
            let octets = ip.octets();
            let is_global_unicast = (octets[0] & 0xe0) == 0x20;
            !(!is_global_unicast
                || ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.to_ipv4().is_some_and(|v4| !is_public_ip(IpAddr::V4(v4)))
                || (octets[0] == 0x20 && octets[1] == 0x01 && (octets[2] & 0xfe) == 0x00)
                || (octets[0] == 0x20
                    && octets[1] == 0x01
                    && octets[2] == 0x0d
                    && octets[3] == 0xb8)
                || (octets[0] == 0x20 && octets[1] == 0x02)
                || (octets[0] == 0x3f && octets[1] == 0xff && (octets[2] & 0xf0) == 0x00))
        }
    }
}

fn encryption_key() -> Result<[u8; 32], AppError> {
    let raw = std::env::var("MUSEHUB_WEBHOOK_ENCRYPTION_KEY").map_err(|_| {
        AppError::Other(
            "MUSEHUB_WEBHOOK_ENCRYPTION_KEY must be set to manage webhook subscriptions".into(),
        )
    })?;
    let bytes = STANDARD.decode(raw.trim()).map_err(|_| {
        AppError::Other("MUSEHUB_WEBHOOK_ENCRYPTION_KEY must be base64-encoded".into())
    })?;
    bytes.try_into().map_err(|_| {
        AppError::Other("MUSEHUB_WEBHOOK_ENCRYPTION_KEY must decode to 32 bytes".into())
    })
}

fn encrypt_secret(secret: &[u8], subscription_id: Uuid) -> Result<Vec<u8>, AppError> {
    let cipher = Aes256Gcm::new_from_slice(&encryption_key()?)
        .map_err(|_| AppError::Other("invalid webhook encryption key".into()))?;
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let aad = subscription_id.as_bytes();
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            aes_gcm::aead::Payload { msg: secret, aad },
        )
        .map_err(|_| AppError::Other("could not encrypt webhook secret".into()))?;
    Ok([nonce_bytes.as_slice(), ciphertext.as_slice()].concat())
}

fn decrypt_secret(
    encrypted: &[u8],
    subscription_id: Uuid,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    if encrypted.len() < 12 {
        return Err("encrypted webhook secret is truncated".into());
    }
    let (nonce, ciphertext) = encrypted.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(&encryption_key()?)?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            aes_gcm::aead::Payload {
                msg: ciphertext,
                aad: subscription_id.as_bytes(),
            },
        )
        .map_err(|_| std::io::Error::other("could not decrypt webhook secret").into())
}

fn format_time(value: time::OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_loopback_destinations() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.2",
            "169.254.169.254",
            "0.1.2.3",
            "100.64.0.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2001::1",
            "2002::1",
            "3fff::1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} should be blocked");
        }
    }

    #[test]
    fn allows_public_destinations() {
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn webhook_retry_schedule_backs_off_and_stops_after_max_attempts() {
        assert_eq!(retry_schedule(1), (false, 2));
        assert_eq!(retry_schedule(7), (false, 128));
        assert_eq!(retry_schedule(MAX_ATTEMPTS), (true, 256));
        assert_eq!(retry_schedule(20), (true, 3600));
    }

    #[tokio::test]
    async fn sends_signed_webhook_over_https_to_isolated_receiver() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        use tokio_rustls::rustls::ServerConfig;
        use tokio_rustls::TlsAcceptor;

        let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca_certificate = ca_params.self_signed(&ca_key).unwrap();
        let server_params = rcgen::CertificateParams::new(vec!["webhook.test".into()]).unwrap();
        let server_key = rcgen::KeyPair::generate().unwrap();
        let server_certificate = server_params
            .signed_by(&server_key, &ca_certificate, &ca_key)
            .unwrap();
        let server_certificate_der = CertificateDer::from(server_certificate.der().to_vec());
        let private_key =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der()));
        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![server_certificate_der], private_key)
            .unwrap();
        let client_certificate = reqwest::Certificate::from_der(ca_certificate.der()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(server_config));
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "receiver closed before complete HTTP request");
                request.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        request.truncate(header_end + 4 + content_length);
                        break;
                    }
                }
            }
            request_tx.send(request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });

        std::env::set_var(
            "MUSEHUB_WEBHOOK_ENCRYPTION_KEY",
            STANDARD.encode([9_u8; 32]),
        );
        let subscription_id = Uuid::new_v4();
        let secret = b"isolated webhook fixture secret";
        let delivery = ClaimedDelivery {
            id: Uuid::new_v4(),
            subscription_id,
            endpoint_url: format!("https://webhook.test:{}/events", address.port()),
            encrypted_secret: encrypt_secret(secret, subscription_id).unwrap(),
            event: serde_json::json!({
                "id": Uuid::new_v4(),
                "type": "issue.created",
                "organization_id": Uuid::new_v4(),
                "repository_id": null,
                "resource_id": Uuid::new_v4(),
                "decision": "allow",
                "occurred_at": "2026-11-19T00:00:00Z"
            }),
            attempt_count: 1,
        };
        let client = reqwest::Client::builder()
            .add_root_certificate(client_certificate)
            .resolve_to_addrs("webhook.test", &[address])
            .redirect(Policy::none())
            .build()
            .unwrap();
        let endpoint = Url::parse(&delivery.endpoint_url).unwrap();

        assert_eq!(
            send_delivery_with_client(&delivery, endpoint, &client)
                .await
                .unwrap(),
            reqwest::StatusCode::NO_CONTENT
        );
        let request = String::from_utf8(request_rx.await.unwrap()).unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(headers.contains("POST /events HTTP/1.1"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"));
        assert!(headers.contains("x-musehub-event: issue.created"));
        assert!(headers.contains(&format!("x-musehub-delivery: {}", delivery.id)));
        let signature = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("x-musehub-signature-256")
                    .then(|| value.trim().strip_prefix("sha256=").unwrap().to_string())
            })
            .unwrap();
        let signature_bytes = signature
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).unwrap();
        mac.update(body.as_bytes());
        mac.verify_slice(&signature_bytes).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            delivery.event
        );
    }
}
