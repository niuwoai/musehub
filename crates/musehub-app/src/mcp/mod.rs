//! Production MCP on `/mcp` — PAT Bearer, domain tools, idempotent writes.

mod auth;
pub mod capabilities;
pub(crate) mod contract_domain;
mod domain;
pub mod envelope;
mod git_read;
mod http;
pub(crate) mod idempotency;
mod pagination;
mod prompts;
mod resource_reads;
mod resources;
mod server;

pub use capabilities::{IMPLEMENTED_TOOLS, V1_RESOURCE_URI_TEMPLATES};
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
        assert_eq!(value["maturity"], "v1_implemented");
        assert!(value.get("production_claim").is_none());
        assert!(value["implemented_tools"].as_array().unwrap().len() > 2);
    }

    #[test]
    fn parse_bearer_requires_prefix() {
        assert_eq!(parse_bearer_token(Some("Bearer mhp_x")), Some("mhp_x"));
        assert_eq!(parse_bearer_token(Some("Basic x")), None);
    }

    #[test]
    fn repo_cursor_roundtrip() {
        use time::OffsetDateTime;
        let ts = OffsetDateTime::now_utc();
        let id = uuid::Uuid::new_v4();
        let c = pagination::encode_repo_cursor(ts, id);
        let (ts2, id2) = pagination::decode_repo_cursor(&c).expect("decode");
        assert_eq!(id, id2);
        assert_eq!(ts, ts2);
        assert!(pagination::decode_repo_cursor("bad").is_err());
    }
}
