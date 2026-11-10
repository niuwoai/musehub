//! MCP write-tool idempotency (`docs/04-mcp-api.md` §2).

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use serde_json::Value;

use crate::error::AppError;

pub fn request_hash(canonical_json: &str) -> String {
    let digest = Sha256::digest(canonical_json.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn validate_idempotency_key(key: &str) -> Result<(), AppError> {
    let key = key.trim();
    if key.len() < 8 || key.len() > 128 {
        return Err(AppError::Validation(
            "idempotency_key must be 8–128 characters".to_string(),
        ));
    }
    Ok(())
}

pub enum IdempotencyAction {
    Proceed { record_id: Uuid },
    Replay(Value),
}

pub async fn begin_idempotent_write(
    pool: &PgPool,
    principal_id: Uuid,
    org_id: Option<Uuid>,
    tool_name: &str,
    idempotency_key: &str,
    canonical_json: &str,
) -> Result<IdempotencyAction, AppError> {
    validate_idempotency_key(idempotency_key)?;
    let hash = request_hash(canonical_json);
    let record_id = Uuid::new_v4();
    let expires = OffsetDateTime::now_utc() + time::Duration::days(7);
    let inserted = sqlx::query(
        "INSERT INTO mcp_idempotency_records
         (id, principal_id, org_id, tool_name, idempotency_key, request_hash, state, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'processing', $7)",
    )
    .bind(record_id)
    .bind(principal_id)
    .bind(org_id)
    .bind(tool_name)
    .bind(idempotency_key)
    .bind(&hash)
    .bind(expires)
    .execute(pool)
    .await;
    if inserted.is_ok() {
        return Ok(IdempotencyAction::Proceed { record_id });
    }
    let row = sqlx::query_as::<_, (Uuid, String, String, Option<Value>)>(
        "SELECT id, request_hash, state, response_json FROM mcp_idempotency_records
         WHERE principal_id = $1 AND tool_name = $2 AND idempotency_key = $3",
    )
    .bind(principal_id)
    .bind(tool_name)
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await?;
    let Some((existing_id, existing_hash, state, response_json)) = row else {
        return Err(AppError::Other("idempotency race".to_string()));
    };
    if existing_hash != hash {
        return Err(AppError::Conflict(
            "idempotency_key reused with different parameters".to_string(),
        ));
    }
    match state.as_str() {
        "completed" => {
            let response = response_json.ok_or_else(|| {
                AppError::Other("idempotency record missing response".to_string())
            })?;
            Ok(IdempotencyAction::Replay(response))
        }
        "processing" => Err(AppError::Validation(
            "operation in progress for this idempotency key".to_string(),
        )),
        _ => Ok(IdempotencyAction::Proceed {
            record_id: existing_id,
        }),
    }
}

pub async fn complete_idempotent_write(
    pool: &PgPool,
    record_id: Uuid,
    response: &Value,
    resource_id: Option<Uuid>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE mcp_idempotency_records
         SET state = 'completed', response_json = $2, resource_id = $3
         WHERE id = $1",
    )
    .bind(record_id)
    .bind(response)
    .bind(resource_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn fail_idempotent_write(pool: &PgPool, record_id: Uuid) -> Result<(), AppError> {
    let _ = sqlx::query("UPDATE mcp_idempotency_records SET state = 'failed' WHERE id = $1")
        .bind(record_id)
        .execute(pool)
        .await;
    Ok(())
}
