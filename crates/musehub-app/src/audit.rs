//! Append-only audit trail (V1 subset). Does not replace tamper-evident archival.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

#[derive(Serialize)]
pub struct AuditEventView {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub repo_id: Option<Uuid>,
    pub actor_id: Uuid,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub decision: String,
    pub metadata: serde_json::Value,
    pub occurred_at: String,
}

#[derive(Deserialize)]
pub struct ListAuditQuery {
    pub limit: Option<i64>,
    pub resource_type: Option<String>,
}

/// Records one audit row inside an open transaction.
#[allow(clippy::too_many_arguments)]
pub async fn record_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Option<Uuid>,
    repo_id: Option<Uuid>,
    actor_id: Uuid,
    initiator_id: Option<Uuid>,
    action: &str,
    resource_type: &str,
    resource_id: Uuid,
    request_id: Option<Uuid>,
    decision: &str,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO audit_events (id, org_id, repo_id, actor_id, initiator_id, action, resource_type, resource_id, request_id, decision, metadata)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(org_id)
    .bind(repo_id)
    .bind(actor_id)
    .bind(initiator_id)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .bind(request_id)
    .bind(decision)
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Records one audit row; failures propagate to the caller when used with `?`.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    pool: &PgPool,
    org_id: Option<Uuid>,
    repo_id: Option<Uuid>,
    actor_id: Uuid,
    initiator_id: Option<Uuid>,
    action: &str,
    resource_type: &str,
    resource_id: Uuid,
    request_id: Option<Uuid>,
    decision: &str,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO audit_events (id, org_id, repo_id, actor_id, initiator_id, action, resource_type, resource_id, request_id, decision, metadata)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(org_id)
    .bind(repo_id)
    .bind(actor_id)
    .bind(initiator_id)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .bind(request_id)
    .bind(decision)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(())
}

pub fn request_id_from_headers(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-request-id")
        .or_else(|| headers.get("X-Request-Id"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
}

pub async fn query_org_audit(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Option<Uuid>,
    resource_type: Option<String>,
    resource_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<AuditEventView>, AppError> {
    let rows = sqlx::query_as::<_, (
        Uuid,
        Option<Uuid>,
        Option<Uuid>,
        Uuid,
        String,
        String,
        Uuid,
        String,
        serde_json::Value,
        time::OffsetDateTime,
    )>(
        "SELECT id, org_id, repo_id, actor_id, action, resource_type, resource_id, decision, metadata, occurred_at
         FROM audit_events
         WHERE org_id = $1
           AND ($2::uuid IS NULL OR repo_id = $2)
           AND ($3::text IS NULL OR resource_type = $3)
           AND ($4::uuid IS NULL OR resource_id = $4)
         ORDER BY occurred_at DESC, id DESC
         LIMIT $5",
    )
    .bind(org_id)
    .bind(repo_id)
    .bind(resource_type)
    .bind(resource_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, o, r, actor, action, rt, rid, decision, meta, at)| AuditEventView {
                id,
                org_id: o,
                repo_id: r,
                actor_id: actor,
                action,
                resource_type: rt,
                resource_id: rid,
                decision,
                metadata: meta,
                occurred_at: at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| at.to_string()),
            },
        )
        .collect())
}

pub async fn list_org_audit(
    State(state): State<std::sync::Arc<crate::AppState>>,
    headers: HeaderMap,
    axum::extract::Path(org): axum::extract::Path<String>,
    Query(query): Query<ListAuditQuery>,
) -> Result<Json<Vec<AuditEventView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = super::org_id_for_member(&state.pool, &org, principal_id).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let items =
        query_org_audit(&state.pool, org_id, None, query.resource_type, None, limit).await?;
    Ok(Json(items))
}
