//! M0 MCP interoperability probe built on the official [`rmcp`] SDK.
//!
//! This crate is **not** the production MCP server. It advertises [`PROBE_SCOPE`]
//! in capability responses so evidence is not mistaken for a full MuseHub MCP
//! deployment.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::any,
    Router,
};
use musehub_core::version;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters, ServerHandler},
    model::{
        CallToolResult, ErrorData, Implementation, JsonObject, ProtocolVersion, ServerCapabilities,
        ServerConfig,
    },
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Frozen `rmcp` crate version used by this probe (not the negotiated MCP wire version).
pub const RMCP_SDK_CRATE_VERSION: &str = "3.8.0";

/// Machine scope marker; must appear in capability data.
pub const PROBE_SCOPE: &str = "m0_mcp_probe";

/// MCP envelope contract revision exercised by this probe.
pub const MCP_CONTRACT_VERSION: i32 = 1;

/// Wire protocol versions this probe server is willing to negotiate.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2026-07-28"];

/// Environment variable holding the Bearer secret for Streamable HTTP (probe only).
pub const HTTP_BEARER_ENV: &str = "MUSEHUB_MCP_PROBE_BEARER";

/// Stable error code for the sample domain error tool.
pub const SAMPLE_ERROR_CODE: &str = "PROBE_SAMPLE_ERROR";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EchoInput {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Unified MuseHub tool envelope (`docs/04-mcp-api.md` §3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolEnvelope {
    pub schema_version: i32,
    pub request_id: String,
    pub data: Option<Value>,
    pub page: Option<Value>,
    pub error: Option<DomainErrorBody>,
}

impl ToolEnvelope {
    pub fn success(data: Value) -> Self {
        Self {
            schema_version: MCP_CONTRACT_VERSION,
            request_id: uuid::Uuid::new_v4().to_string(),
            data: Some(data),
            page: None,
            error: None,
        }
    }

    pub fn domain_error(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            schema_version: MCP_CONTRACT_VERSION,
            request_id: uuid::Uuid::new_v4().to_string(),
            data: None,
            page: None,
            error: Some(DomainErrorBody {
                code: code.into(),
                message: message.into(),
                retryable,
                details: Some(json!({ "probe_scope": PROBE_SCOPE })),
            }),
        }
    }
}

/// Probe MCP server shared by stdio and Streamable HTTP transports.
#[derive(Debug, Clone)]
pub struct ProbeServer {
    tool_router: ToolRouter<Self>,
}

impl Default for ProbeServer {
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    pub fn capability_payload(transport: &str) -> Value {
        json!({
            "service_version": version(),
            "contract_version": MCP_CONTRACT_VERSION,
            "probe_scope": PROBE_SCOPE,
            "rmcp_sdk_crate_version": RMCP_SDK_CRATE_VERSION,
            "supported_protocol_versions": SUPPORTED_PROTOCOL_VERSIONS,
            "tool_groups": ["m0_probe"],
            "limits": {
                "idempotency_write_tools_verified": false
            },
            "transport": transport,
            "production_claim": false
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ProbeServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "musehub-mcp-probe",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "M0 rmcp interoperability probe. Not a production MuseHub MCP endpoint.",
            )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28])
    }
}

#[tool_router(router = tool_router)]
impl ProbeServer {
    #[tool(
        name = "musehub_capabilities_get",
        description = "Return probe capabilities and explicit non-production scope",
        annotations(read_only_hint = true)
    )]
    async fn musehub_capabilities_get(&self) -> Result<CallToolResult, ErrorData> {
        // Transport is filled by echo/callers in matrix tests when needed.
        Ok(CallToolResult::structured(json!(ToolEnvelope::success(
            Self::capability_payload("unspecified")
        ))))
    }

    #[tool(
        name = "musehub_probe_echo",
        description = "Echo a message inside the MuseHub structured envelope",
        annotations(read_only_hint = true)
    )]
    async fn musehub_probe_echo(
        &self,
        Parameters(EchoInput { message }): Parameters<EchoInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return Err(ErrorData::invalid_params(
                "message must be non-empty",
                Some(json!({ "field": "message" })),
            ));
        }
        Ok(CallToolResult::structured(json!(ToolEnvelope::success(
            json!({
                "echo": trimmed,
                "probe_scope": PROBE_SCOPE,
            })
        ))))
    }

    #[tool(
        name = "musehub_probe_error_sample",
        description = "Return a sample domain error envelope (tool-level, not protocol error)",
        annotations(read_only_hint = true)
    )]
    async fn musehub_probe_error_sample(&self) -> Result<CallToolResult, ErrorData> {
        let envelope = ToolEnvelope::domain_error(
            SAMPLE_ERROR_CODE,
            "This is an intentional sample domain error for envelope verification.",
            false,
        );
        Ok(CallToolResult::structured_error(json!(envelope)))
    }
}

/// Expected read-only tool names advertised by the probe.
pub const EXPECTED_PROBE_TOOLS: &[&str] = &[
    "musehub_capabilities_get",
    "musehub_probe_echo",
    "musehub_probe_error_sample",
];

/// Build an Axum router that serves Streamable HTTP at `/mcp` behind Bearer auth.
pub fn http_probe_router(bearer: Arc<str>, cancel: CancellationToken) -> Router {
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };

    let service = StreamableHttpService::new(
        || Ok(ProbeServer::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_cancellation_token(cancel),
    );

    Router::new()
        .nest_service("/mcp", service)
        .route("/healthz", any(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            bearer,
            require_bearer_for_mcp,
        ))
}

/// Bind an ephemeral loopback address and serve [`http_probe_router`] until `cancel` fires.
pub async fn serve_http_probe(
    bearer: Arc<str>,
    cancel: CancellationToken,
) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = http_probe_router(bearer, cancel.child_token());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                cancel.cancelled().await;
            })
            .await;
    });
    Ok(address)
}

async fn require_bearer_for_mcp(
    axum::extract::State(expected): axum::extract::State<Arc<str>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !request.uri().path().starts_with("/mcp") {
        return Ok(next.run(request).await);
    }
    let provided = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if provided != expected.as_ref() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

/// Convenience for matrix tests: build JSON arguments for [`musehub_probe_echo`].
pub fn echo_arguments(message: &str) -> JsonObject {
    let mut map = JsonObject::new();
    map.insert("message".to_string(), json!(message));
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_success_shape_matches_contract() {
        let envelope = ToolEnvelope::success(json!({ "k": 1 }));
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["schema_version"], MCP_CONTRACT_VERSION);
        assert!(value["data"].is_object());
        assert!(value["error"].is_null());
        assert!(value["page"].is_null());
        assert!(value["request_id"].is_string());
    }

    #[test]
    fn probe_scope_is_stable() {
        assert_eq!(PROBE_SCOPE, "m0_mcp_probe");
    }
}
