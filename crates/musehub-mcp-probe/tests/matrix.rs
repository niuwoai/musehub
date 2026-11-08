//! MCP client matrix: **two independent SDKs** against the same `ProbeServer`.
//!
//! - `rust-rmcp` 3.8.0 (`TokioChildProcess` / `StreamableHttpClientTransport`)
//! - `python-mcp-sdk` 2.3.0 (official Python MCP SDK, subprocess runner)

mod python_harness;

use std::collections::BTreeSet;
use std::sync::Arc;

use http::HeaderValue;
use musehub_mcp_probe::{
    echo_arguments, ToolEnvelope, EXPECTED_PROBE_TOOLS, MCP_CONTRACT_VERSION, PROBE_SCOPE,
    RMCP_SDK_CRATE_VERSION, SAMPLE_ERROR_CODE, SUPPORTED_PROTOCOL_VERSIONS,
};
use python_harness::{
    python_http_bearer_reject, python_http_matrix, python_stdio_matrix,
    PYTHON_CLIENT_IMPLEMENTATION, PYTHON_CLIENT_NAME, PYTHON_MCP_SDK_VERSION,
};
use rmcp::{
    model::{CallToolRequestParams, ClientCapabilities, Implementation, ProtocolVersion},
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
        TokioChildProcess,
    },
    ClientLifecycleMode, ClientServiceExt, ServiceExt,
};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const RUST_CLIENT_IMPLEMENTATION: &str = "rust-rmcp";
const RUST_CLIENT_NAME: &str = "musehub-mcp-matrix-rust-rmcp";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TransportReport {
    transport: String,
    negotiated_protocol: String,
    tools_discovered: Vec<String>,
    checks: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ClientReport {
    client_implementation: String,
    client_sdk_version: String,
    client_name: String,
    transports: Vec<TransportReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct MatrixDocument {
    probe_scope: String,
    server_implementation: String,
    server_rmcp_version: String,
    mcp_contract_version: i32,
    supported_protocol_versions: Vec<String>,
    write_idempotency_verified: bool,
    m0_mcp_interop_matrix_passed: bool,
    m0_overall_passed: bool,
    clients: Vec<ClientReport>,
}

async fn exercise_rust_rmcp_client<H: rmcp::handler::client::ClientHandler + Send + 'static>(
    transport_label: &str,
    client: rmcp::service::RunningService<rmcp::RoleClient, H>,
    include_bearer_check: bool,
) -> TransportReport {
    let negotiated = client
        .peer_info()
        .map(|info| info.protocol_version.to_string())
        .expect("peer should be initialized");

    let tools = client.list_all_tools().await.expect("tools/list");
    let mut names: Vec<String> = tools
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();

    for expected in EXPECTED_PROBE_TOOLS {
        assert!(
            names.iter().any(|name| name == expected),
            "missing tool {expected} on {transport_label}"
        );
    }

    let caps = client
        .call_tool(CallToolRequestParams::new("musehub_capabilities_get"))
        .await
        .expect("capabilities");
    let caps_envelope: ToolEnvelope = serde_json::from_value(
        caps.structured_content
            .clone()
            .expect("capabilities structured content"),
    )
    .expect("capabilities envelope");
    assert_eq!(caps_envelope.schema_version, MCP_CONTRACT_VERSION);

    let echo = client
        .call_tool(
            CallToolRequestParams::new("musehub_probe_echo")
                .with_arguments(echo_arguments("matrix-ping")),
        )
        .await
        .expect("echo");
    let echo_envelope: ToolEnvelope = serde_json::from_value(
        echo.structured_content
            .clone()
            .expect("echo structured content"),
    )
    .expect("echo envelope");
    assert_eq!(
        echo_envelope
            .data
            .as_ref()
            .and_then(|data| data.get("echo"))
            .and_then(|value| value.as_str()),
        Some("matrix-ping")
    );

    assert!(
        client
            .call_tool(
                CallToolRequestParams::new("musehub_probe_echo")
                    .with_arguments(echo_arguments("   "))
            )
            .await
            .is_err(),
        "empty echo must be rejected"
    );

    let sample = client
        .call_tool(CallToolRequestParams::new("musehub_probe_error_sample"))
        .await
        .expect("error sample tool should run");
    assert!(
        sample.is_error.unwrap_or(false),
        "expected tool-level error"
    );
    let sample_envelope: ToolEnvelope = serde_json::from_value(
        sample
            .structured_content
            .clone()
            .expect("sample structured content"),
    )
    .expect("sample envelope");
    assert_eq!(
        sample_envelope
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some(SAMPLE_ERROR_CODE)
    );

    client.cancel().await.expect("cancel client");

    let mut checks = BTreeSet::from([
        "initialize".to_string(),
        "list_tools".to_string(),
        "capabilities_get".to_string(),
        "probe_echo".to_string(),
        "invalid_echo_rejected".to_string(),
        "error_sample_envelope".to_string(),
    ]);
    if include_bearer_check {
        checks.insert("bearer_auth_gate".to_string());
    }

    assert!(SUPPORTED_PROTOCOL_VERSIONS.contains(&negotiated.as_str()));

    TransportReport {
        transport: transport_label.to_string(),
        negotiated_protocol: negotiated,
        tools_discovered: names,
        checks,
    }
}

fn rust_client_config() -> rmcp::model::ClientConfig {
    rmcp::model::ClientConfig::new(
        ClientCapabilities::builder().build(),
        Implementation::new(RUST_CLIENT_NAME, env!("CARGO_PKG_VERSION")),
    )
}

fn python_run_to_transport(run: python_harness::PythonMatrixRun) -> TransportReport {
    assert_eq!(run.client_implementation, PYTHON_CLIENT_IMPLEMENTATION);
    assert_eq!(run.client_sdk_version, PYTHON_MCP_SDK_VERSION);
    assert_eq!(run.client_name, PYTHON_CLIENT_NAME);
    assert!(run.passed);
    TransportReport {
        transport: run.transport,
        negotiated_protocol: run.negotiated_protocol,
        tools_discovered: run.tools_discovered,
        checks: run.checks.into_iter().collect(),
    }
}

#[tokio::test]
async fn rust_rmcp_stdio_transport_matrix() {
    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_musehub-mcp-probe-stdio")))
            .expect("spawn stdio probe");

    let client = rust_client_config()
        .serve(transport)
        .await
        .expect("rust rmcp stdio client");

    let _ = exercise_rust_rmcp_client("stdio", client, false).await;
}

#[tokio::test]
async fn rust_rmcp_streamable_http_transport_matrix() {
    let cancel = CancellationToken::new();
    let bearer: Arc<str> = Arc::from("matrix-bearer-token-7f3a");
    let address = musehub_mcp_probe::serve_http_probe(bearer.clone(), cancel.child_token())
        .await
        .expect("bind http probe");

    let mut headers = std::collections::HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {bearer}")).expect("header"),
    );

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp"))
            .custom_headers(headers),
    );

    let client = rust_client_config()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("rust rmcp http client");

    let _ = exercise_rust_rmcp_client("streamable_http", client, true).await;
    cancel.cancel();
}

#[test]
fn python_mcp_sdk_stdio_transport_matrix() {
    let run = python_stdio_matrix(env!("CARGO_BIN_EXE_musehub-mcp-probe-stdio"));
    let _ = python_run_to_transport(run);
}

#[tokio::test]
async fn python_mcp_sdk_streamable_http_transport_matrix() {
    let cancel = CancellationToken::new();
    let bearer: Arc<str> = Arc::from("python-matrix-bearer-82c1");
    let address = musehub_mcp_probe::serve_http_probe(bearer.clone(), cancel.child_token())
        .await
        .expect("bind");
    let url = format!("http://{address}/mcp");
    let bearer_copy = bearer.to_string();
    let run = tokio::task::spawn_blocking(move || python_http_matrix(&url, &bearer_copy))
        .await
        .expect("spawn_blocking join");
    let _ = python_run_to_transport(run);
    cancel.cancel();
}

#[tokio::test]
async fn streamable_http_bearer_rejected_for_rust_and_python() {
    let cancel = CancellationToken::new();
    let bearer: Arc<str> = Arc::from("expected-bearer-9ac1");
    let address = musehub_mcp_probe::serve_http_probe(bearer.clone(), cancel.child_token())
        .await
        .expect("bind");
    let url = format!("http://{address}/mcp");

    let transport_no_auth = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.as_str()),
    );
    assert!(
        rust_client_config()
            .serve_with_lifecycle(
                transport_no_auth,
                ClientLifecycleMode::Auto {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    legacy_version: Some(ProtocolVersion::V_2025_11_25),
                },
            )
            .await
            .is_err(),
        "rust rmcp: missing bearer"
    );

    let url_copy = url.clone();
    tokio::task::spawn_blocking(move || python_http_bearer_reject(&url_copy))
        .await
        .expect("spawn_blocking join");
    cancel.cancel();
}

#[tokio::test]
async fn rust_rmcp_unknown_tool_returns_error() {
    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_musehub-mcp-probe-stdio")))
            .expect("spawn stdio probe");

    let client = rust_client_config().serve(transport).await.expect("client");

    assert!(client
        .call_tool(CallToolRequestParams::new("musehub_no_such_tool"))
        .await
        .is_err());
    client.cancel().await.expect("cancel");
}

#[test]
fn independent_client_implementations_are_not_the_same_sdk() {
    assert_ne!(RUST_CLIENT_IMPLEMENTATION, PYTHON_CLIENT_IMPLEMENTATION);
    assert_ne!(RMCP_SDK_CRATE_VERSION, PYTHON_MCP_SDK_VERSION);
}

#[test]
fn matrix_document_matches_repo_evidence_schema() {
    let doc: MatrixDocument =
        serde_json::from_str(include_str!("../../../docs/m0-rmcp-matrix.json"))
            .expect("docs/m0-rmcp-matrix.json");

    assert_eq!(doc.probe_scope, PROBE_SCOPE);
    assert_eq!(doc.server_rmcp_version, RMCP_SDK_CRATE_VERSION);
    assert!(!doc.write_idempotency_verified);
    assert!(!doc.m0_overall_passed);
    assert!(doc.m0_mcp_interop_matrix_passed);
    assert_eq!(doc.clients.len(), 2);
    let impls: BTreeSet<_> = doc
        .clients
        .iter()
        .map(|c| c.client_implementation.as_str())
        .collect();
    assert!(impls.contains("rust-rmcp"));
    assert!(impls.contains("python-mcp-sdk"));
    for client in &doc.clients {
        assert_eq!(client.transports.len(), 2);
        let transports: BTreeSet<_> = client
            .transports
            .iter()
            .map(|t| t.transport.as_str())
            .collect();
        assert!(transports.contains("stdio"));
        assert!(transports.contains("streamable_http"));
    }
}
