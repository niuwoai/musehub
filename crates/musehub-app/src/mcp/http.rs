use std::sync::Arc;

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
    {
        let mut guard = state.mcp_pat_slot.write().await;
        *guard = Some(pat);
    }
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
