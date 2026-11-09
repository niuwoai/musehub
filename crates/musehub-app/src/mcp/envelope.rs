use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::PRODUCTION_SCOPE;

pub const MCP_CONTRACT_VERSION: i32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

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
            request_id: Uuid::new_v4().to_string(),
            data: Some(data),
            page: None,
            error: None,
        }
    }

    pub fn success_page(data: Value, page: Value) -> Self {
        Self {
            schema_version: MCP_CONTRACT_VERSION,
            request_id: Uuid::new_v4().to_string(),
            data: Some(data),
            page: Some(page),
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
            request_id: Uuid::new_v4().to_string(),
            data: None,
            page: None,
            error: Some(DomainErrorBody {
                code: code.into(),
                message: message.into(),
                retryable,
                details: Some(json!({ "mcp_scope": PRODUCTION_SCOPE })),
            }),
        }
    }
}

pub fn app_err_envelope(err: crate::error::AppError) -> ToolEnvelope {
    match err {
        crate::error::AppError::NotFound => {
            ToolEnvelope::domain_error("NOT_FOUND", "resource not found", false)
        }
        crate::error::AppError::Forbidden => {
            ToolEnvelope::domain_error("PERMISSION_DENIED", "permission denied", false)
        }
        crate::error::AppError::Unauthorized => {
            ToolEnvelope::domain_error("PERMISSION_DENIED", "unauthorized", false)
        }
        crate::error::AppError::Validation(msg) => {
            ToolEnvelope::domain_error("VALIDATION_FAILED", msg, false)
        }
        crate::error::AppError::Conflict(msg) => {
            ToolEnvelope::domain_error("IDEMPOTENCY_CONFLICT", msg, false)
        }
        other => ToolEnvelope::domain_error("INTERNAL", other.to_string(), false),
    }
}
