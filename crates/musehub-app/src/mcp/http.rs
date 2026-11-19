use std::sync::Arc;

use tokio::sync::OwnedMutexGuard;

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, Method, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tokio_util::sync::CancellationToken;

use super::server::ProductionMcpServer;
use crate::pat_auth;
use crate::pat_policy;
use crate::AppState;

pub fn parse_bearer_token(authorization_header: Option<&str>) -> Option<&str> {
    authorization_header?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

async fn install_request_pat(
    gate: &Arc<tokio::sync::Mutex<()>>,
    slot: &Arc<tokio::sync::RwLock<Option<pat_auth::PatContext>>>,
    pat: pat_auth::PatContext,
) -> OwnedMutexGuard<()> {
    let guard = gate.clone().lock_owned().await;
    *slot.write().await = Some(pat);
    guard
}

pub fn product_mcp_router(state: Arc<AppState>, cancel: CancellationToken) -> Router {
    let state_for_service = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(ProductionMcpServer::new(state_for_service.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_cancellation_token(cancel),
    );

    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, require_pat_for_mcp))
}

/// Every MCP HTTP request (except CORS preflight) must present a valid PAT Bearer token.
/// `mcp-session-id` is transport routing only; it is never accepted without Bearer.
async fn require_pat_for_mcp(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !request.uri().path().starts_with("/mcp") {
        return Ok(next.run(request).await);
    }
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let Some(token) = parse_bearer_token(header) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let pat = pat_auth::authenticate_pat_context(&state.pool, token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    if pat_policy::is_runner_only_pat(&pat.scopes) {
        return Err(StatusCode::FORBIDDEN);
    }
    // Only POST carries MCP requests that can reach handlers using the PAT slot.
    // GET/DELETE may manage long-lived transport streams and do not read it.
    if request.method() != Method::POST {
        return Ok(next.run(request).await);
    }
    // Handlers currently obtain their PAT through AppState. Hold this gate from
    // installing the authenticated identity until the MCP request has fully
    // completed, so concurrent HTTP requests cannot observe each other's PAT.
    let _request_guard =
        install_request_pat(&state.mcp_request_gate, &state.mcp_pat_slot, pat).await;
    Ok(next.run(request).await)
}

pub fn merge_product_mcp(api: Router, state: Arc<AppState>) -> Router {
    if !super::product_mcp_enabled() {
        return api;
    }
    let cancel = CancellationToken::new();
    api.merge(product_mcp_router(state, cancel))
}

pub fn product_mcp_enabled() -> bool {
    std::env::var("MUSEHUB_MCP_PRODUCT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(principal_id: uuid::Uuid) -> pat_auth::PatContext {
        pat_auth::PatContext {
            token_id: uuid::Uuid::new_v4(),
            principal_id,
            org_id: None,
            scopes: vec!["repo:read".to_string()],
            scope_mode: "all".to_string(),
        }
    }

    #[tokio::test]
    async fn concurrent_requests_cannot_replace_the_active_pat_mid_request() {
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let slot = Arc::new(tokio::sync::RwLock::new(None));
        let first_id = uuid::Uuid::new_v4();
        let second_id = uuid::Uuid::new_v4();

        let first_guard = install_request_pat(&gate, &slot, pat(first_id)).await;
        let second_gate = Arc::clone(&gate);
        let second_slot = Arc::clone(&slot);
        let second = tokio::spawn(async move {
            install_request_pat(&second_gate, &second_slot, pat(second_id)).await
        });

        tokio::task::yield_now().await;
        assert_eq!(
            slot.read().await.as_ref().map(|pat| pat.principal_id),
            Some(first_id)
        );

        drop(first_guard);
        let second_guard = second.await.expect("second request acquires gate");
        assert_eq!(
            slot.read().await.as_ref().map(|pat| pat.principal_id),
            Some(second_id)
        );
        drop(second_guard);
    }
}
