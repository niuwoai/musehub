//! REST issue list/detail/update (ISS-01).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::list_cursor::{decode_issue_cursor, encode_issue_cursor};
use crate::mcp::contract_domain::{self, CriterionInput};
use crate::mcp::RepoAccess;
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

#[derive(Serialize)]
pub struct IssueAssigneeOption {
    pub handle: String,
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

const MAX_LABELS: usize = 20;
const MAX_LABEL_LEN: usize = 50;

const MAX_CLOSE_REASON_LEN: usize = 200;

pub(crate) fn validate_close_reason(reason: &str) -> Result<(), AppError> {
    if reason.is_empty() || reason.len() > MAX_CLOSE_REASON_LEN {
        return Err(AppError::Validation("invalid close_reason".into()));
    }
    Ok(())
}

pub(crate) fn validate_labels(labels: &[String]) -> Result<(), AppError> {
    if labels.len() > MAX_LABELS {
        return Err(AppError::Validation(format!(
            "at most {MAX_LABELS} labels allowed"
        )));
    }
    for label in labels {
        if label.is_empty() || label.len() > MAX_LABEL_LEN {
            return Err(AppError::Validation("invalid label length".into()));
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AppError::Validation("invalid label characters".into()));
        }
    }
    Ok(())
}

/// Assignee must be an active org member **and** have repository access (grant or org admin/owner).
async fn repo_assignee_id(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    handle: &str,
) -> Result<Uuid, AppError> {
    let row = sqlx::query_as::<_, (Uuid,)>(
        "SELECT p.id FROM principals p
         JOIN memberships m ON m.principal_id = p.id AND m.org_id = $2
         WHERE p.handle_normalized = LOWER($1)
           AND p.status = 'active'
           AND m.status = 'active'",
    )
    .bind(handle)
    .bind(org_id)
    .fetch_optional(pool)
    .await?;
    let Some((id,)) = row else {
        return Err(AppError::Validation("assignee not eligible".into()));
    };
    match crate::grants::ensure_principal_repo_access(pool, org_id, repo_id, id).await {
        Ok(()) => Ok(id),
        Err(AppError::Forbidden) => Err(AppError::Validation("assignee not eligible".into())),
        Err(e) => Err(e),
    }
}

#[derive(Serialize)]
pub struct AcceptanceCriterion {
    pub ordinal: i32,
    pub description: String,
    pub verification_method: String,
    pub required: bool,
}

#[derive(Serialize)]
pub struct IssuePlanSummary {
    pub id: Uuid,
    pub version: i32,
    pub state: String,
    pub body: String,
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
    pub current_revision_id: Option<Uuid>,
    pub revision: Option<i32>,
    pub criteria: Vec<AcceptanceCriterion>,
    pub plan: Option<IssuePlanSummary>,
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

/// Active org members who may be assigned (repo grant or org owner/admin), same rule as PATCH assignee.
pub async fn list_issue_assignee_options(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
) -> Result<Json<Vec<IssueAssigneeOption>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT p.handle FROM principals p
         JOIN memberships m ON m.principal_id = p.id AND m.org_id = $2 AND m.status = 'active'
         WHERE p.status = 'active'
           AND (
             m.role IN ('owner', 'admin')
             OR EXISTS (
               SELECT 1 FROM repository_grants g
               WHERE g.repo_id = $1 AND g.principal_id = p.id
             )
           )
         ORDER BY p.handle",
    )
    .bind(repo_id)
    .bind(org_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(handle,)| IssueAssigneeOption { handle })
            .collect(),
    ))
}

pub async fn list_issues_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Query(q): Query<IssueListQuery>,
) -> Result<Json<IssueListPage>, AppError> {
    let limit = q.limit.clamp(1, 100);
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;

    let mut cursor_number: Option<i64> = None;
    let mut cursor_id: Option<Uuid> = None;
    if let Some(c) = &q.cursor {
        let (n, id) = decode_issue_cursor(c)?;
        cursor_number = Some(n);
        cursor_id = Some(id);
    }

    let assignee_id: Option<Uuid> = if let Some(handle) = &q.assignee {
        Some(repo_assignee_id(&state.pool, org_id, repo_id, handle).await?)
    } else {
        None
    };

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
        page_rows
            .last()
            .map(|(id, number, ..)| encode_issue_cursor(*number, *id))
    } else {
        None
    };
    Ok(Json(IssueListPage {
        items: page_rows
            .into_iter()
            .map(
                |(id, number, title, issue_state, labels, assignee_handle)| IssueListItem {
                    id,
                    number,
                    title,
                    state: issue_state,
                    labels,
                    assignee_handle,
                },
            )
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
            Option<Uuid>,
        ),
    >(
        "SELECT i.id, i.number, i.title, i.body, i.state, i.labels, i.assignee_id,
                (SELECT handle FROM principals WHERE id = i.assignee_id),
                i.close_reason,
                (SELECT handle FROM principals WHERE id = i.author_id),
                i.current_revision_id
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
        current_revision_id,
    )) = row
    else {
        return Err(AppError::NotFound);
    };
    let (revision, criteria, plan) = issue_revision_meta(&state.pool, current_revision_id).await?;
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
        current_revision_id,
        revision,
        criteria,
        plan,
    }))
}

async fn issue_revision_meta(
    pool: &sqlx::PgPool,
    revision_id: Option<Uuid>,
) -> Result<
    (
        Option<i32>,
        Vec<AcceptanceCriterion>,
        Option<IssuePlanSummary>,
    ),
    AppError,
> {
    let Some(revision_id) = revision_id else {
        return Ok((None, Vec::new(), None));
    };
    let rev_row = sqlx::query_as::<_, (i32,)>("SELECT revision FROM issue_revisions WHERE id = $1")
        .bind(revision_id)
        .fetch_optional(pool)
        .await?;
    let Some((revision,)) = rev_row else {
        return Ok((None, Vec::new(), None));
    };
    let crit_values = contract_domain::criteria_for_revision(pool, revision_id).await?;
    let criteria: Vec<AcceptanceCriterion> = crit_values
        .into_iter()
        .filter_map(|v| {
            Some(AcceptanceCriterion {
                ordinal: v.get("ordinal")?.as_i64()? as i32,
                description: v.get("description")?.as_str()?.to_string(),
                verification_method: v.get("verification_method")?.as_str()?.to_string(),
                required: v.get("required")?.as_bool()?,
            })
        })
        .collect();
    let plan_row = sqlx::query_as::<_, (Uuid, i32, String, String)>(
        "SELECT id, version, state, body FROM implementation_plans
         WHERE issue_revision_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(revision_id)
    .fetch_optional(pool)
    .await?;
    let plan = plan_row.map(|(id, version, state, body)| IssuePlanSummary {
        id,
        version,
        state,
        body,
    });
    Ok((Some(revision), criteria, plan))
}

#[derive(Deserialize)]
pub struct CriterionBody {
    pub description: String,
    #[serde(default = "default_verification")]
    pub verification_method: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_verification() -> String {
    "manual".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
pub struct PutCriteriaRequest {
    pub expected_version: i32,
    pub criteria: Vec<CriterionBody>,
}

pub async fn put_issue_criteria(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo, number)): Path<(String, String, i64)>,
    Json(body): Json<PutCriteriaRequest>,
) -> Result<Json<IssueDetail>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id) =
        super::repo_org_ids_for_member(&state.pool, &org, &repo, principal_id).await?;
    let issue =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE repo_id = $1 AND number = $2")
            .bind(repo_id)
            .bind(number)
            .fetch_optional(&state.pool)
            .await?;
    let Some((issue_id,)) = issue else {
        return Err(AppError::NotFound);
    };
    let access = RepoAccess {
        org_id,
        repo_id,
        org_slug: org.clone(),
        repo_slug: repo.clone(),
    };
    let inputs: Vec<CriterionInput> = body
        .criteria
        .into_iter()
        .map(|c| CriterionInput {
            description: c.description,
            verification_method: c.verification_method,
            required: c.required,
        })
        .collect();
    contract_domain::issue_criteria_replace(
        &state,
        &access,
        principal_id,
        issue_id,
        body.expected_version,
        &inputs,
    )
    .await?;
    get_issue(State(state), headers, Path((org, repo, number))).await
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
    let issue =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE repo_id = $1 AND number = $2")
            .bind(repo_id)
            .bind(number)
            .fetch_optional(&state.pool)
            .await?;
    let Some((issue_id,)) = issue else {
        return Err(AppError::NotFound);
    };

    let mut assignee_id: Option<Uuid> = None;
    if let Some(labels) = &body.labels {
        validate_labels(labels)?;
    }
    if let Some(handle) = &body.assignee_handle {
        if handle.is_empty() {
            assignee_id = None;
        } else {
            assignee_id = Some(repo_assignee_id(&state.pool, org_id, repo_id, handle).await?);
        }
    }

    if let Some(state_val) = &body.state {
        if !matches!(state_val.as_str(), "open" | "closed" | "in_progress") {
            return Err(AppError::Validation("invalid state".into()));
        }
        if state_val == "closed" {
            let reason = body
                .close_reason
                .as_deref()
                .ok_or_else(|| AppError::Validation("close_reason required when closing".into()))?;
            validate_close_reason(reason)?;
        }
    } else if let Some(reason) = &body.close_reason {
        validate_close_reason(reason)?;
        let (current_state,): (String,) = sqlx::query_as("SELECT state FROM issues WHERE id = $1")
            .bind(issue_id)
            .fetch_one(&state.pool)
            .await?;
        if current_state != "closed" {
            return Err(AppError::Validation(
                "close_reason requires issue state closed".into(),
            ));
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
                    WHEN $5 = 'open' THEN NULL
                    WHEN $5 = 'closed' THEN COALESCE($7, close_reason)
                    WHEN $7 IS NOT NULL THEN $7
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
        let rev: (i64,) = sqlx::query_as(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM issue_revisions WHERE issue_id = $1",
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_validation() {
        assert!(validate_labels(&["bug".into(), "p0".into()]).is_ok());
        assert!(validate_labels(&["ok_label".into()]).is_ok());
        assert!(validate_labels(&["".into()]).is_err());
        assert!(validate_labels(&["bad!".into()]).is_err());
        assert!(validate_labels(&vec!["x".repeat(51)]).is_err());
        assert!(validate_labels(&vec!["l".repeat(50); 21]).is_err());
    }

    #[test]
    fn close_reason_validation() {
        assert!(validate_close_reason("completed").is_ok());
        assert!(validate_close_reason("").is_err());
        assert!(validate_close_reason(&"x".repeat(201)).is_err());
    }
}
