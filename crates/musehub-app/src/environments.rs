//! REST: repository deployment environments (DEP-02 read path).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::AppState;

#[derive(Serialize)]
pub struct EnvironmentView {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub requires_approval: bool,
    pub row_version: i64,
}

pub async fn list_repo_environments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
) -> Result<Json<Vec<EnvironmentView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, bool, i64)>(
        "SELECT id, slug, name, requires_approval, row_version
         FROM environments WHERE repo_id = $1 ORDER BY slug",
    )
    .bind(repo_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, slug, name, requires_approval, row_version)| EnvironmentView {
                    id,
                    slug,
                    name,
                    requires_approval,
                    row_version,
                },
            )
            .collect(),
    ))
}
