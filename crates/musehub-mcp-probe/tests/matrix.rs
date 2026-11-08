//! Dual-transport rmcp matrix: stdio child-process client vs Streamable HTTP client.
//!
//! Both clients talk to the same [`ProbeServer`] implementation through different
//! transports. This is not an in-process worker self-loop.

use std::collections::BTreeSet;
use std::sync::Arc;

use http::HeaderValue;
use musehub_mcp_probe::{
    echo_arguments, ToolEnvelope, EXPECTED_PROBE_TOOLS, MCP_CONTRACT_VERSION, PROBE_SCOPE,
    RMCP_SDK_CRATE_VERSION, SAMPLE_ERROR_CODE, SUPPORTED_PROTOCOL_VERSIONS,
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

const STDIO_CLIENT_NAME: &str = "musehub-mcp-matrix-stdio-client";
const HTTP_CLIENT_NAME: &str = "musehub-mcp-matrix-http-client";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TransportReport {
    transport: String,
    client_implementation: String,
    negotiated_protocol: String,
    tools_discovered: Vec<String>,
    checks: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct MatrixReport {
    rmcp_sdk_crate_version: String,
    mcp_contract_version: i32,
    probe_scope: String,
    supported_protocol_versions: Vec<String>,
    write_idempotency_verified: bool,
    m0_overall_passed: bool,
    transports: Vec<TransportReport>,
}

async fn exercise_client<H: rmcp::handler::client::ClientHandler + Send + 'static>(
    transport_label: &str,
    client_name: &str,
    client: rmcp::service::RunningService<rmcp::RoleClient, H>,
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
    let caps_value = caps
        .structured_content
        .clone()
        .expect("capabilities structured content");
    let caps_envelope: ToolEnvelope =
        serde_json::from_value(caps_value).expect("capabilities envelope");
    assert_eq!(caps_envelope.schema_version, MCP_CONTRACT_VERSION);
    assert_eq!(
        caps_envelope
            .data
            .as_ref()
            .and_then(|data| data.get("probe_scope"))
            .and_then(|value| value.as_str()),
        Some(PROBE_SCOPE)
    );

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

    let invalid = client
        .call_tool(
            CallToolRequestParams::new("musehub_probe_echo").with_arguments(echo_arguments("   ")),
        )
        .await;
    assert!(
        invalid.is_err(),
        "empty echo should be a protocol/params error, not success"
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
    if transport_label == "streamable_http" {
        checks.insert("bearer_auth_gate".to_string());
    }

    TransportReport {
        transport: transport_label.to_string(),
        client_implementation: client_name.to_string(),
        negotiated_protocol: negotiated,
        tools_discovered: names,
        checks,
    }
}

fn client_config(name: &str) -> rmcp::model::ClientConfig {
    rmcp::model::ClientConfig::new(
        ClientCapabilities::builder().build(),
        Implementation::new(name, env!("CARGO_PKG_VERSION")),
    )
}

#[tokio::test]
async fn stdio_child_process_client_matrix() {
    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_musehub-mcp-probe-stdio")))
            .expect("spawn stdio probe");

    let client = client_config(STDIO_CLIENT_NAME)
        .serve(transport)
        .await
        .expect("stdio client should initialize");

    let report = exercise_client("stdio", STDIO_CLIENT_NAME, client).await;
    assert_ne!(report.client_implementation, HTTP_CLIENT_NAME);
    assert!(SUPPORTED_PROTOCOL_VERSIONS.contains(&report.negotiated_protocol.as_str()));
}

#[tokio::test]
async fn streamable_http_client_matrix() {
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

    let client = client_config(HTTP_CLIENT_NAME)
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("http client should initialize");

    let report = exercise_client("streamable_http", HTTP_CLIENT_NAME, client).await;
    assert_ne!(report.client_implementation, STDIO_CLIENT_NAME);
    assert!(SUPPORTED_PROTOCOL_VERSIONS.contains(&report.negotiated_protocol.as_str()));

    cancel.cancel();
}

#[tokio::test]
async fn streamable_http_rejects_missing_or_wrong_bearer() {
    let cancel = CancellationToken::new();
    let bearer: Arc<str> = Arc::from("expected-bearer-9ac1");
    let address = musehub_mcp_probe::serve_http_probe(bearer.clone(), cancel.child_token())
        .await
        .expect("bind");

    let transport_no_auth = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let missing = client_config(HTTP_CLIENT_NAME)
        .serve_with_lifecycle(
            transport_no_auth,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await;
    assert!(missing.is_err(), "missing bearer must not initialize");

    let mut headers = std::collections::HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_static("Bearer wrong-token"),
    );
    let transport_wrong = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp"))
            .custom_headers(headers),
    );
    let wrong = client_config(HTTP_CLIENT_NAME)
        .serve_with_lifecycle(
            transport_wrong,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await;
    assert!(wrong.is_err(), "wrong bearer must not initialize");

    cancel.cancel();
}

#[tokio::test]
async fn unknown_tool_returns_service_error() {
    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_musehub-mcp-probe-stdio")))
            .expect("spawn stdio probe");

    let client = client_config(STDIO_CLIENT_NAME)
        .serve(transport)
        .await
        .expect("client");

    let result = client
        .call_tool(CallToolRequestParams::new("musehub_no_such_tool"))
        .await;
    assert!(result.is_err(), "unknown tool should error");

    client.cancel().await.expect("cancel");
}

#[test]
fn matrix_report_contract_matches_repo_evidence_doc() {
    let report = MatrixReport {
        rmcp_sdk_crate_version: RMCP_SDK_CRATE_VERSION.to_string(),
        mcp_contract_version: MCP_CONTRACT_VERSION,
        probe_scope: PROBE_SCOPE.to_string(),
        supported_protocol_versions: SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .map(|version| (*version).to_string())
            .collect(),
        write_idempotency_verified: false,
        m0_overall_passed: false,
        transports: vec![
            TransportReport {
                transport: "stdio".to_string(),
                client_implementation: STDIO_CLIENT_NAME.to_string(),
                negotiated_protocol: "<set by cargo test -p musehub-mcp-probe --test matrix>"
                    .into(),
                tools_discovered: EXPECTED_PROBE_TOOLS
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
                checks: BTreeSet::from([
                    "initialize".into(),
                    "list_tools".into(),
                    "capabilities_get".into(),
                    "probe_echo".into(),
                    "invalid_echo_rejected".into(),
                    "error_sample_envelope".into(),
                ]),
            },
            TransportReport {
                transport: "streamable_http".to_string(),
                client_implementation: HTTP_CLIENT_NAME.to_string(),
                negotiated_protocol: "<set by cargo test -p musehub-mcp-probe --test matrix>"
                    .into(),
                tools_discovered: EXPECTED_PROBE_TOOLS
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
                checks: BTreeSet::from([
                    "initialize".into(),
                    "list_tools".into(),
                    "capabilities_get".into(),
                    "probe_echo".into(),
                    "invalid_echo_rejected".into(),
                    "error_sample_envelope".into(),
                    "bearer_auth_gate".into(),
                ]),
            },
        ],
    };

    let doc: MatrixReport = serde_json::from_str(include_str!("../../../docs/m0-rmcp-matrix.json"))
        .expect("docs/m0-rmcp-matrix.json must match MatrixReport");
    assert_eq!(doc.rmcp_sdk_crate_version, report.rmcp_sdk_crate_version);
    assert_eq!(doc.write_idempotency_verified, false);
    assert_eq!(doc.m0_overall_passed, false);
    assert_eq!(doc.transports.len(), 2);
}
