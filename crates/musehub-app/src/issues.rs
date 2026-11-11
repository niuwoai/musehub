//! REST issue list/detail/update (ISS-01).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::list_cursor::{decode_issue_cursor, encode_issue_cursor};
use crate::AppState;

#[derive(Serialize)]
pub struct IssueListItem {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub assignee_handle: Option<String>,
}

#[derive(Serialize)]
pub struct IssueListPage {
    pub items: Vec<IssueListItem>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Deserialize)]
pub struct IssueListQuery {
    pub state: Option<String>,
    pub label: Option<String>,
    pub assignee: Option<String>,
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Serialize)]
pub struct IssueDetail {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub labels: Vec<String>,
    pub assignee_id: Option<Uuid>,
    pub assignee_handle: Option<String>,
    pub author_handle: String,
    pub close_reason: Option<String>,
}

#[derive(Deserialize)]
pub struct PatchIssueRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
    pub labels: Option<Vec<String>>,
    pub assignee_handle: Option<String>,
    pub close_reason: Option<String>,
}

pub async fn list_issues_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Query(q): Query<IssueListQuery>,
) -> Result<Json<IssueListPage>, AppError> {
    let limit = q.limit.clamp(1, 100);
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;

    let mut cursor_number: Option<i64> = None;
    let mut cursor_id: Option<Uuid> = None;
    if let Some(c) = &q.cursor {
        let (n, id) = decode_issue_cursor(c)?;
        cursor_number = Some(n);
        cursor_id = Some(id);
    }

    let mut assignee_id: Option<Uuid> = None;
    if let Some(handle) = &q.assignee {
        let row = sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM principals WHERE handle_normalized = LOWER($1)",
        )
        .bind(handle)
        .fetch_optional(&state.pool)
        .await?;
        assignee_id = row.map(|r| r.0);
    }

    let rows = sqlx::query_as::<_, (Uuid, i64, String, String, Vec<String>, Option<String>)>(
        "SELECT i.id, i.number, i.title, i.state, i.labels,
                (SELECT handle FROM principals WHERE id = i.assignee_id)
         FROM issues i
         WHERE i.repo_id = $1
           AND ($2::text IS NULL OR i.state = $2)
           AND ($3::text IS NULL OR $3 = ANY(i.labels))
           AND ($4::uuid IS NULL OR i.assignee_id = $4)
           AND ($5::bigint IS NULL OR i.number < $5 OR (i.number = $5 AND i.id < $6))
         ORDER BY i.number DESC, i.id DESC
         LIMIT $7",
    )
    .bind(repo_id)
    .bind(q.state.as_deref())
    .bind(q.label.as_deref())
    .bind(assignee_id)
    .bind(cursor_number)
    .bind(cursor_id)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;

    let has_more = rows.len() as i64 > limit;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more {
        page_rows.last().map(|(id, number, ..)| encode_issue_cursor(*number, *id))
    } else {
        None
    };
    Ok(Json(IssueListPage {
        items: page_rows
            .into_iter()
            .map(|(id, number, title, issue_state, labels, assignee_handle)| IssueListItem {
                id,
                number,
                title,
                state: issue_state,
                labels,
                assignee_handle,
            })
            .collect(),
        next_cursor,
        has_more,
    }))
}

pub async fn get_issue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, number)): Path<(String, String, i64)>,
) -> Result<Json<IssueDetail>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            i64,
            String,
            String,
            String,
            Vec<String>,
            Option<Uuid>,
            Option<String>,
            Option<String>,
            String,
        ),
    >(
        "SELECT i.id, i.number, i.title, i.body, i.state, i.labels, i.assignee_id,
                (SELECT handle FROM principals WHERE id = i.assignee_id),
                i.close_reason,
                (SELECT handle FROM principals WHERE id = i.author_id)
         FROM issues i WHERE i.repo_id = $1 AND i.number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((
        id,
        num,
        title,
        body,
        issue_state,
        labels,
        assignee_id,
        assignee_handle,
        close_reason,
        author_handle,
    )) = row
    else {
        return Err(AppError::NotFound);
    };
    Ok(Json(IssueDetail {
        id,
        number: num,
        title,
        body,
        state: issue_state,
        labels,
        assignee_id,
        assignee_handle,
        author_handle,
        close_reason,
    }))
}

pub async fn patch_issue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, number)): Path<(String, String, i64)>,
    Json(body): Json<PatchIssueRequest>,
) -> Result<Json<IssueDetail>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let issue = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM issues WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((issue_id,)) = issue else {
        return Err(AppError::NotFound);
    };

    let mut assignee_id: Option<Uuid> = None;
    if let Some(handle) = &body.assignee_handle {
        if handle.is_empty() {
            assignee_id = None;
        } else {
            let row = sqlx::query_as::<_, (Uuid,)>(
                "SELECT id FROM principals WHERE handle_normalized = LOWER($1)",
            )
            .bind(handle)
            .fetch_optional(&state.pool)
            .await?;
            assignee_id = Some(row.ok_or_else(|| AppError::Validation("unknown assignee".into()))?.0);
        }
    }

    if let Some(state_val) = &body.state {
        if !matches!(state_val.as_str(), "open" | "closed" | "in_progress") {
            return Err(AppError::Validation("invalid state".into()));
        }
        if state_val == "closed" && body.close_reason.is_none() {
            return Err(AppError::Validation("close_reason required when closing".into()));
        }
    }

    let title = body.title.as_deref();
    let issue_body = body.body.as_deref();
    if title.is_some()
        || issue_body.is_some()
        || body.state.is_some()
        || body.labels.is_some()
        || body.close_reason.is_some()
    {
        sqlx::query(
            "UPDATE issues SET
                title = COALESCE($3, title),
                body = COALESCE($4, body),
                state = COALESCE($5, state),
                labels = COALESCE($6, labels),
                close_reason = CASE
                    WHEN $5 = 'closed' THEN COALESCE($7, close_reason)
                    WHEN $5 = 'open' THEN NULL
                    ELSE close_reason
                END,
                updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND repo_id = $2",
        )
        .bind(issue_id)
        .bind(repo_id)
        .bind(title)
        .bind(issue_body)
        .bind(body.state.as_deref())
        .bind(body.labels.as_ref())
        .bind(body.close_reason.as_deref())
        .execute(&state.pool)
        .await?;
    }
    if let Some(handle) = &body.assignee_handle {
        if handle.is_empty() {
            sqlx::query("UPDATE issues SET assignee_id = NULL WHERE id = $1")
                .bind(issue_id)
                .execute(&state.pool)
                .await?;
        } else if let Some(aid) = assignee_id {
            sqlx::query("UPDATE issues SET assignee_id = $2 WHERE id = $1")
                .bind(issue_id)
                .bind(aid)
                .execute(&state.pool)
                .await?;
        }
    }

    if title.is_some() || issue_body.is_some() {
        let (title_now, body_now): (String, String) =
            sqlx::query_as("SELECT title, body FROM issues WHERE id = $1")
                .bind(issue_id)
                .fetch_one(&state.pool)
                .await?;
        let rev: (i64,) =
            sqlx::query_as("SELECT COALESCE(MAX(revision), 0) + 1 FROM issue_revisions WHERE issue_id = $1")
                .bind(issue_id)
                .fetch_one(&state.pool)
                .await?;
        let revision_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO issue_revisions (id, org_id, repo_id, issue_id, revision, title, body, author_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(revision_id)
        .bind(org_id)
        .bind(repo_id)
        .bind(issue_id)
        .bind(rev.0)
        .bind(&title_now)
        .bind(&body_now)
        .bind(principal_id)
        .execute(&state.pool)
        .await?;
        sqlx::query("UPDATE issues SET current_revision_id = $2 WHERE id = $1")
            .bind(issue_id)
            .bind(revision_id)
            .execute(&state.pool)
            .await?;
    }

    get_issue(State(state), headers, Path((org, repo, number))).await
}
