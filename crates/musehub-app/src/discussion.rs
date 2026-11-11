//! REST discussion threads for issues and pull requests.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::AppError;
use crate::AppState;

#[derive(Serialize)]
pub struct DiscussionCommentView {
    pub id: Uuid,
    pub body: String,
    pub author_handle: String,
    pub anchor: Option<Value>,
    pub outdated: bool,
    pub resolved: bool,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct PostCommentRequest {
    pub body: String,
    pub anchor: Option<Value>,
}

async fn issue_id_for_number(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    number: i64,
) -> Result<Uuid, AppError> {
    let row = sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE repo_id = $1 AND number = $2")
        .bind(repo_id)
        .bind(number)
        .fetch_optional(pool)
        .await?;
    row.map(|r| r.0).ok_or(AppError::NotFound)
}

async fn pr_id_for_number(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    number: i64,
) -> Result<(Uuid, i64), AppError> {
    let row = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, row_version FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(pool)
    .await?;
    row.ok_or(AppError::NotFound)
}

pub async fn list_issue_comments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, number)): Path<(String, String, i64)>,
) -> Result<Json<Vec<DiscussionCommentView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let issue_id = issue_id_for_number(&state.pool, repo_id, number).await?;
    list_comments(&state.pool, repo_id, "issue", issue_id).await
}

pub async fn post_issue_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, number)): Path<(String, String, i64)>,
    Json(body): Json<PostCommentRequest>,
) -> Result<(StatusCode, Json<DiscussionCommentView>), AppError> {
    if body.body.trim().is_empty() {
        return Err(AppError::Validation("body required".into()));
    }
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let issue_id = issue_id_for_number(&state.pool, repo_id, number).await?;
    insert_comment(
        &state.pool,
        org_id,
        repo_id,
        "issue",
        issue_id,
        principal_id,
        &body.body,
        body.anchor,
    )
    .await
}

pub async fn list_pr_comments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, number)): Path<(String, String, i64)>,
) -> Result<Json<Vec<DiscussionCommentView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let (pr_id, _) = pr_id_for_number(&state.pool, repo_id, number).await?;
    list_comments(&state.pool, repo_id, "pr", pr_id).await
}

pub async fn post_pr_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, number)): Path<(String, String, i64)>,
    Json(body): Json<PostCommentRequest>,
) -> Result<(StatusCode, Json<DiscussionCommentView>), AppError> {
    if body.body.trim().is_empty() {
        return Err(AppError::Validation("body required".into()));
    }
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let (pr_id, row_version) = pr_id_for_number(&state.pool, repo_id, number).await?;
    let mut anchor = body.anchor.unwrap_or(Value::Null);
    if anchor.is_object() {
        if let Some(obj) = anchor.as_object_mut() {
            obj.insert(
                "pr_revision_id".to_string(),
                Value::Number(row_version.into()),
            );
        }
    }
    insert_comment(
        &state.pool,
        org_id,
        repo_id,
        "pr",
        pr_id,
        principal_id,
        &body.body,
        Some(anchor),
    )
    .await
}

pub async fn mark_pr_comments_outdated(pool: &sqlx::PgPool, pr_id: Uuid, new_row_version: i64) {
    let _ = sqlx::query(
        "UPDATE discussion_comments SET outdated = true, updated_at = CURRENT_TIMESTAMP
         WHERE target_kind = 'pr' AND target_id = $1 AND outdated = false
           AND anchor IS NOT NULL
           AND (anchor->>'pr_revision_id')::bigint IS NOT NULL
           AND (anchor->>'pr_revision_id')::bigint < $2",
    )
    .bind(pr_id)
    .bind(new_row_version)
    .execute(pool)
    .await;
}

async fn list_comments(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    target_kind: &str,
    target_id: Uuid,
) -> Result<Json<Vec<DiscussionCommentView>>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<Value>, bool, bool, time::OffsetDateTime)>(
        "SELECT c.id, c.body, p.handle, c.anchor, c.outdated, c.resolved, c.created_at
         FROM discussion_comments c
         JOIN principals p ON p.id = c.author_id
         WHERE c.repo_id = $1 AND c.target_kind = $2 AND c.target_id = $3
         ORDER BY c.created_at ASC",
    )
    .bind(repo_id)
    .bind(target_kind)
    .bind(target_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, body, author_handle, anchor, outdated, resolved, created_at)| {
                DiscussionCommentView {
                    id,
                    body,
                    author_handle,
                    anchor,
                    outdated,
                    resolved,
                    created_at: created_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                }
            })
            .collect(),
    ))
}

async fn insert_comment(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    target_kind: &str,
    target_id: Uuid,
    author_id: Uuid,
    body: &str,
    anchor: Option<Value>,
) -> Result<(StatusCode, Json<DiscussionCommentView>), AppError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO discussion_comments (id, org_id, repo_id, target_kind, target_id, author_id, body, anchor)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(org_id)
    .bind(repo_id)
    .bind(target_kind)
    .bind(target_id)
    .bind(author_id)
    .bind(body.trim())
    .bind(anchor)
    .execute(pool)
    .await?;
    let author: (String,) =
        sqlx::query_as("SELECT handle FROM principals WHERE id = $1")
            .bind(author_id)
            .fetch_one(pool)
            .await?;
    Ok((
        StatusCode::CREATED,
        Json(DiscussionCommentView {
            id,
            body: body.trim().to_string(),
            author_handle: author.0,
            anchor: None,
            outdated: false,
            resolved: false,
            created_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        }),
    ))
}
