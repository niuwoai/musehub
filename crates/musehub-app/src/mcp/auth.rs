use uuid::Uuid;

use crate::error::AppError;
use crate::pat_auth::{self, PatContext};
use crate::AppState;

#[derive(Clone, Debug)]
pub struct RepoAccess {
    pub org_id: Uuid,
    pub repo_id: Uuid,
    #[allow(dead_code)]
    pub org_slug: String,
    #[allow(dead_code)]
    pub repo_slug: String,
}

pub async fn active_pat(state: &AppState) -> Result<PatContext, AppError> {
    let guard = state.mcp_pat_slot.read().await;
    guard.clone().ok_or(AppError::Unauthorized)
}

pub fn require_scope(pat: &PatContext, scope: &str) -> Result<(), AppError> {
    if !pat_auth::scope_allows(&pat.scopes, scope) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

pub fn deny_runner_only_pat(pat: &PatContext) -> Result<(), AppError> {
    if crate::pat_policy::is_runner_only_pat(&pat.scopes) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

pub async fn resolve_repo(
    state: &AppState,
    pat: &PatContext,
    org_id: Uuid,
    repository_id: Uuid,
) -> Result<RepoAccess, AppError> {
    let org_row = sqlx::query_as::<_, (String,)>("SELECT slug FROM organizations WHERE id = $1")
        .bind(org_id)
        .fetch_optional(&state.pool)
        .await?;
    let Some((org_slug,)) = org_row else {
        return Err(AppError::NotFound);
    };
    crate::grants::ensure_pat_repo_access(&state.pool, pat, org_id, repository_id).await?;
    let repo = sqlx::query_as::<_, (String,)>(
        "SELECT slug FROM repositories WHERE id = $1 AND org_id = $2",
    )
    .bind(repository_id)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((repo_slug,)) = repo else {
        return Err(AppError::NotFound);
    };
    Ok(RepoAccess {
        org_slug,
        org_id,
        repo_id: repository_id,
        repo_slug,
    })
}
