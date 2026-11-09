//! Production MCP on `/mcp` — PAT Bearer, domain tools, idempotent writes.

mod auth;
pub mod capabilities;
mod domain;
pub mod envelope;
mod http;
mod idempotency;
mod server;

pub use capabilities::IMPLEMENTED_TOOLS;
pub use envelope::ToolEnvelope;
pub use http::{merge_product_mcp, parse_bearer_token, product_mcp_enabled};

pub const PRODUCTION_SCOPE: &str = "m1_product_mcp";
pub const RMCP_SDK_CRATE_VERSION: &str = "3.8.0";
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2026-07-28"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_lists_tools_without_blanket_production_claim() {
        let value = capabilities::capability_payload();
        assert_eq!(value["mcp_scope"], PRODUCTION_SCOPE);
        assert_eq!(value["maturity"], "partial_implementation");
        assert!(value.get("production_claim").is_none());
        assert!(value["implemented_tools"].as_array().unwrap().len() > 2);
    }

    #[test]
    fn parse_bearer_requires_prefix() {
        assert_eq!(parse_bearer_token(Some("Bearer mhp_x")), Some("mhp_x"));
        assert_eq!(parse_bearer_token(Some("Basic x")), None);
    }
}
