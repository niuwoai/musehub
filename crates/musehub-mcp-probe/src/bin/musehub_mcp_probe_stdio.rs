//! Stdio entrypoint for the M0 MCP interoperability probe.
//!
//! Spawned as a child process by matrix tests (`TokioChildProcess`), not as the
//! production MuseHub MCP proxy.

use musehub_mcp_probe::ProbeServer;
use rmcp::{transport::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = ProbeServer::new();
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
