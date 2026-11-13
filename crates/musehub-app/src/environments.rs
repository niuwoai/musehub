//! REST: repository deployment environments (DEP-02).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::grants;
use crate::AppState;

#[derive(Serialize)]
pub struct EnvironmentView {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub requires_approval: bool,
    pub row_version: i64,
}

#[derive(Deserialize)]
pub struct CreateEnvironmentRequest {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub requires_approval: bool,
}

#[derive(Deserialize)]
pub struct PatchEnvironmentRequest {
    pub name: Option<String>,
    pub requires_approval: Option<bool>,
    pub expected_row_version: i64,
}

fn normalize_env_slug(slug: &str) -> Result<String, AppError> {
    let slug = slug.trim().to_lowercase();
    if slug.is_empty()
        || slug.len() > 64
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(AppError::Validation("invalid environment slug".into()));
    }
    Ok(slug)
}

async fn require_repo_env_admin(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    principal_id: Uuid,
) -> Result<(), AppError> {
    if grants::is_org_privileged(pool, org_id, principal_id).await? {
        return Ok(());
    }
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM repository_grants WHERE repo_id = $1 AND principal_id = $2",
    )
    .bind(repo_id)
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    let Some((role,)) = row else {
        return Err(AppError::Forbidden);
    };
    if role == "maintainer" {
        return Ok(());
    }
    Err(AppError::Forbidden)
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

pub async fn create_repo_environment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Json(body): Json<CreateEnvironmentRequest>,
) -> Result<(StatusCode, Json<EnvironmentView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    require_repo_env_admin(&state.pool, org_id, repo_id, principal_id).await?;
    let slug = normalize_env_slug(&body.slug)?;
    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation("environment name required".into()));
    }
    let id = Uuid::new_v4();
    let result = sqlx::query(
        "INSERT INTO environments (id, org_id, repo_id, slug, name, requires_approval)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&slug)
    .bind(name)
    .bind(body.requires_approval)
    .execute(&state.pool)
    .await;
    if let Err(e) = result {
        if let sqlx::Error::Database(db) = &e {
            if db.code().as_deref() == Some("23505") {
                return Err(AppError::Conflict("environment slug already exists".into()));
            }
        }
        return Err(e.into());
    }
    Ok((
        StatusCode::CREATED,
        Json(EnvironmentView {
            id,
            slug,
            name: name.to_string(),
            requires_approval: body.requires_approval,
            row_version: 1,
        }),
    ))
}

pub async fn patch_repo_environment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, slug)): Path<(String, String, String)>,
    Json(body): Json<PatchEnvironmentRequest>,
) -> Result<Json<EnvironmentView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    require_repo_env_admin(&state.pool, org_id, repo_id, principal_id).await?;
    let env_slug = normalize_env_slug(&slug)?;
    let name = body
        .name
        .as_ref()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    if body.name.is_some() && name.is_none() {
        return Err(AppError::Validation("environment name required".into()));
    }
    let row = sqlx::query_as::<_, (Uuid, String, bool, i64)>(
        "UPDATE environments SET
            name = COALESCE($4, name),
            requires_approval = COALESCE($5, requires_approval),
            row_version = row_version + 1
         WHERE repo_id = $1 AND slug = $2 AND row_version = $3
         RETURNING id, name, requires_approval, row_version",
    )
    .bind(repo_id)
    .bind(&env_slug)
    .bind(body.expected_row_version)
    .bind(name.as_deref())
    .bind(body.requires_approval)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, name, requires_approval, row_version)) = row else {
        return Err(AppError::Conflict(
            "environment row_version mismatch".into(),
        ));
    };
    Ok(Json(EnvironmentView {
        id,
        slug: env_slug,
        name,
        requires_approval,
        row_version,
    }))
}
