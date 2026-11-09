//! PAT scope issuance and runner-token policy.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

const RUNNER_ALLOWED: &[&str] = &["checks:write", "repo:read"];

pub fn validate_scope_issuance(scopes: &[String]) -> Result<(), AppError> {
    for scope in scopes {
        if scope.is_empty() || scope.len() > 64 {
            return Err(AppError::Validation("invalid scope".to_string()));
        }
    }
    if scopes.iter().any(|s| s == "checks:write") {
        if scopes
            .iter()
            .any(|s| s == "repo:write" || s == "repo:admin")
        {
            return Err(AppError::Validation(
                "checks:write runner tokens cannot include repo:write or repo:admin".to_string(),
            ));
        }
        for scope in scopes {
            if !RUNNER_ALLOWED.contains(&scope.as_str()) {
                return Err(AppError::Validation(format!(
                    "checks:write tokens may only include: {}",
                    RUNNER_ALLOWED.join(", ")
                )));
            }
        }
    }
    Ok(())
}

pub async fn require_org_admin_to_issue_runner_token(
    pool: &PgPool,
    org_id: Uuid,
    principal_id: Uuid,
    scopes: &[String],
) -> Result<(), AppError> {
    if !scopes.iter().any(|s| s == "checks:write") {
        return Ok(());
    }
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    let Some((role,)) = row else {
        return Err(AppError::Forbidden);
    };
    if role != "owner" && role != "admin" {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

pub fn is_runner_only_pat(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == "checks:write")
        && !scopes
            .iter()
            .any(|s| s == "repo:write" || s == "repo:admin")
}

pub async fn resolve_runner_token_principal(
    pool: &PgPool,
    org_id: Uuid,
    handle: &str,
) -> Result<Uuid, AppError> {
    let norm = handle.trim().to_lowercase();
    if norm.is_empty() {
        return Err(AppError::Validation("runner_handle required".to_string()));
    }
    let row =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = $1")
            .bind(&norm)
            .fetch_optional(pool)
            .await?;
    let Some((principal_id,)) = row else {
        return Err(AppError::NotFound);
    };
    if principal_is_privileged_org_member(pool, org_id, principal_id).await? {
        return Err(AppError::Validation(
            "runner tokens must be issued for a non-admin org member".to_string(),
        ));
    }
    let member = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(principal_id)
    .fetch_one(pool)
    .await?;
    if member.0 == 0 {
        return Err(AppError::Validation(
            "runner_handle must belong to an active org member".to_string(),
        ));
    }
    Ok(principal_id)
}

pub async fn principal_is_privileged_org_member(
    pool: &PgPool,
    org_id: Uuid,
    principal_id: Uuid,
) -> Result<bool, AppError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    Ok(matches!(
        row,
        Some((role,)) if role == "owner" || role == "admin"
    ))
}
