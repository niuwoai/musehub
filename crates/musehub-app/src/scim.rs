//! Organization-scoped SCIM 2.0 user provisioning.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::{hash_token, normalize_handle, session_token};
use crate::error::AppError;
use crate::AppState;

const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const LIST_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const ERROR_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:Error";

#[derive(Debug)]
pub struct ScimApiError(AppError);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScimErrorBody {
    schemas: [&'static str; 1],
    detail: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scim_type: Option<&'static str>,
}

impl From<AppError> for ScimApiError {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}

impl From<sqlx::Error> for ScimApiError {
    fn from(error: sqlx::Error) -> Self {
        Self(AppError::Database(error))
    }
}

impl IntoResponse for ScimApiError {
    fn into_response(self) -> Response {
        let (status, scim_type, detail) = match self.0 {
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Some("invalidToken"),
                "Invalid or expired bearer token".to_string(),
            ),
            AppError::Forbidden => (
                StatusCode::FORBIDDEN,
                None,
                "Operation is not permitted".to_string(),
            ),
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                None,
                "Resource not found".to_string(),
            ),
            AppError::Validation(detail) => {
                let scim_type = if detail.to_ascii_lowercase().contains("filter") {
                    "invalidFilter"
                } else {
                    "invalidValue"
                };
                (StatusCode::BAD_REQUEST, Some(scim_type), detail)
            }
            AppError::Conflict(detail)
                if detail.to_ascii_lowercase().contains("already assigned") =>
            {
                (StatusCode::CONFLICT, Some("uniqueness"), detail)
            }
            AppError::Conflict(detail) => (StatusCode::BAD_REQUEST, Some("mutability"), detail),
            AppError::DatabaseNotPrimary => (
                StatusCode::SERVICE_UNAVAILABLE,
                None,
                "Service temporarily unavailable".to_string(),
            ),
            AppError::Database(_) | AppError::Other(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
                "Internal server error".to_string(),
            ),
        };
        scim_error_response(status, detail, scim_type)
    }
}

fn scim_error_response(
    status: StatusCode,
    detail: String,
    scim_type: Option<&'static str>,
) -> Response {
    let body = ScimErrorBody {
        schemas: [ERROR_SCHEMA],
        detail,
        status: status.as_u16().to_string(),
        scim_type,
    };
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/scim+json"),
    );
    response
}

pub async fn normalize_scim_extractor_errors(request: Request, next: Next) -> Response {
    const SCIM_PATH_PREFIX: &str = "/api/v1/scim/v2/organizations/";
    let is_scim_request = request.uri().path().starts_with(SCIM_PATH_PREFIX);
    let response = next.run(request).await;
    if !is_scim_request
        || (!response.status().is_client_error() && !response.status().is_server_error())
    {
        return response;
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/scim+json"))
    {
        return response;
    }

    let status = response.status();
    let scim_type = match status {
        StatusCode::BAD_REQUEST => Some("invalidSyntax"),
        StatusCode::UNPROCESSABLE_ENTITY => Some("invalidValue"),
        _ => None,
    };
    let detail = match status {
        StatusCode::NOT_FOUND => "SCIM resource was not found",
        StatusCode::METHOD_NOT_ALLOWED => "SCIM operation is not supported for this resource",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "SCIM request has an unsupported content type",
        _ => "SCIM request could not be parsed",
    };
    scim_error_response(status, detail.to_string(), scim_type)
}

const MAX_PAGE_SIZE: u32 = 500;
const POSTGRES_UNIQUE_VIOLATION_CODE: &str = "23505";
const PRINCIPAL_HANDLE_UNIQUE_CONSTRAINT: &str = "uk_principals_handle_normalized";
const SCIM_EXTERNAL_ID_UNIQUE_CONSTRAINT: &str = "uk_scim_identities_org_external_id";
const SCIM_GROUP_EXTERNAL_ID_UNIQUE_CONSTRAINT: &str = "uk_scim_groups_org_external_id";
const DEFAULT_PAGE_SIZE: u32 = 100;

fn scim_user_creation_conflict(constraint: Option<&str>) -> Option<AppError> {
    match constraint {
        Some(PRINCIPAL_HANDLE_UNIQUE_CONSTRAINT) => Some(AppError::Conflict(
            "userName is already assigned to an account".into(),
        )),
        Some(SCIM_EXTERNAL_ID_UNIQUE_CONSTRAINT) => Some(AppError::Conflict(
            "externalId is already assigned in this organization".into(),
        )),
        _ => None,
    }
}

fn map_scim_user_creation_error(error: sqlx::Error) -> ScimApiError {
    let constraint = error.as_database_error().and_then(|database_error| {
        (database_error.code().as_deref() == Some(POSTGRES_UNIQUE_VIOLATION_CODE))
            .then(|| database_error.constraint().map(str::to_owned))
            .flatten()
    });
    scim_user_creation_conflict(constraint.as_deref())
        .map(ScimApiError)
        .unwrap_or_else(|| ScimApiError(AppError::Database(error)))
}

fn map_scim_group_write_error(error: sqlx::Error) -> ScimApiError {
    let constraint = error.as_database_error().and_then(|database_error| {
        (database_error.code().as_deref() == Some(POSTGRES_UNIQUE_VIOLATION_CODE))
            .then(|| database_error.constraint().map(str::to_owned))
            .flatten()
    });
    if constraint.as_deref() == Some(SCIM_GROUP_EXTERNAL_ID_UNIQUE_CONSTRAINT) {
        ScimApiError(AppError::Conflict(
            "externalId is already assigned to a Group in this organization".into(),
        ))
    } else {
        ScimApiError(AppError::Database(error))
    }
}

#[derive(Deserialize)]
pub struct CreateScimToken {
    pub expires_in_days: Option<i64>,
}

#[derive(Serialize)]
pub struct CreatedScimToken {
    pub id: Uuid,
    pub token: String,
    pub token_prefix: String,
    pub expires_at: String,
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<CreateScimToken>,
) -> Result<(StatusCode, Json<CreatedScimToken>), AppError> {
    let creator_id = super::require_session(&state.pool, &headers).await?;
    let org_id = crate::members::require_org_admin(&state.pool, &org, creator_id).await?;
    let expires_in_days = body.expires_in_days.unwrap_or(90);
    if !(1..=365).contains(&expires_in_days) {
        return Err(AppError::Validation(
            "SCIM token expiry must be between 1 and 365 days".into(),
        ));
    }
    let token = session_token();
    let token_prefix = token.chars().take(8).collect::<String>();
    let id = Uuid::new_v4();
    let expires_at = OffsetDateTime::now_utc() + time::Duration::days(expires_in_days);
    let expires_at_text = format_scim_timestamp(expires_at)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO scim_tokens (id, org_id, created_by, token_prefix, token_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(org_id)
    .bind(creator_id)
    .bind(&token_prefix)
    .bind(hash_token(&token))
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    crate::audit::record_in_tx(
        &mut tx,
        Some(org_id),
        None,
        creator_id,
        Some(creator_id),
        "scim.token.create",
        "scim_token",
        id,
        crate::audit::request_id_from_headers(&headers),
        "allow",
        serde_json::json!({ "token_prefix": token_prefix, "expires_at": expires_at_text }),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedScimToken {
            id,
            token,
            token_prefix,
            expires_at: expires_at_text,
        }),
    ))
}

#[derive(Serialize)]
pub struct ScimTokenView {
    pub id: Uuid,
    pub token_prefix: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    pub last_used_at: Option<String>,
}

pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Json<Vec<ScimTokenView>>, AppError> {
    let actor_id = super::require_session(&state.pool, &headers).await?;
    let org_id = crate::members::require_org_admin(&state.pool, &org, actor_id).await?;
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            OffsetDateTime,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
        ),
    >(
        "SELECT id, token_prefix, expires_at, revoked_at, last_used_at
         FROM scim_tokens WHERE org_id = $1 ORDER BY created_at DESC",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await?;
    let tokens = rows
        .into_iter()
        .map(|(id, token_prefix, expires_at, revoked_at, last_used_at)| {
            Ok(ScimTokenView {
                id,
                token_prefix,
                expires_at: format_scim_timestamp(expires_at)?,
                revoked_at: revoked_at.map(format_scim_timestamp).transpose()?,
                last_used_at: last_used_at.map(format_scim_timestamp).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(tokens))
}

pub async fn revoke_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, token_id)): Path<(String, Uuid)>,
) -> Result<StatusCode, AppError> {
    let actor_id = super::require_session(&state.pool, &headers).await?;
    let org_id = crate::members::require_org_admin(&state.pool, &org, actor_id).await?;
    let mut tx = state.pool.begin().await?;
    let result = sqlx::query(
        "UPDATE scim_tokens SET revoked_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND org_id = $2 AND revoked_at IS NULL",
    )
    .bind(token_id)
    .bind(org_id)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    crate::audit::record_in_tx(
        &mut tx,
        Some(org_id),
        None,
        actor_id,
        Some(actor_id),
        "scim.token.revoke",
        "scim_token",
        token_id,
        crate::audit::request_id_from_headers(&headers),
        "allow",
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Copy)]
struct ScimContext {
    org_id: Uuid,
    actor_id: Uuid,
    token_id: Uuid,
}

async fn authenticate(
    pool: &PgPool,
    org_slug: &str,
    headers: &HeaderMap,
) -> Result<ScimContext, AppError> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| value.len() >= 32)
        .ok_or(AppError::Unauthorized)?;
    let org = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM organizations WHERE slug_normalized = $1 AND status = 'active'",
    )
    .bind(normalize_handle(org_slug))
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let token = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        "SELECT t.id, t.org_id, t.created_by
         FROM scim_tokens t
         JOIN principals p ON p.id = t.created_by
         WHERE t.token_hash = $1 AND t.org_id = $2
           AND t.revoked_at IS NULL AND t.expires_at > CURRENT_TIMESTAMP
           AND p.status = 'active'",
    )
    .bind(hash_token(raw))
    .bind(org.0)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::Unauthorized)?;
    sqlx::query("UPDATE scim_tokens SET last_used_at = CURRENT_TIMESTAMP WHERE id = $1")
        .bind(token.0)
        .execute(pool)
        .await?;
    Ok(ScimContext {
        org_id: token.1,
        actor_id: token.2,
        token_id: token.0,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimUser {
    pub schemas: Vec<&'static str>,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub user_name: String,
    pub display_name: String,
    pub active: bool,
    pub meta: ScimMeta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimMeta {
    pub resource_type: &'static str,
    pub created: String,
    pub last_modified: String,
    pub location: String,
}

type UserDbRow = (
    Uuid,
    Option<String>,
    String,
    String,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

struct UserRow {
    id: Uuid,
    external_id: Option<String>,
    handle: String,
    display_name: String,
    active: bool,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl From<UserDbRow> for UserRow {
    fn from(
        (id, external_id, handle, display_name, active, created_at, updated_at): UserDbRow,
    ) -> Self {
        Self {
            id,
            external_id,
            handle,
            display_name,
            active,
            created_at,
            updated_at,
        }
    }
}

fn format_scim_timestamp(value: OffsetDateTime) -> Result<String, AppError> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| AppError::Other(format!("format SCIM timestamp: {error}")))
}

fn to_scim_user(row: UserRow, base: &str) -> Result<ScimUser, AppError> {
    let id = row.id.to_string();
    Ok(ScimUser {
        schemas: vec![USER_SCHEMA],
        meta: ScimMeta {
            resource_type: "User",
            created: format_scim_timestamp(row.created_at)?,
            last_modified: format_scim_timestamp(row.updated_at)?,
            location: format!("{base}/Users/{id}"),
        },
        id,
        external_id: row.external_id,
        user_name: row.handle,
        display_name: row.display_name,
        active: row.active,
    })
}

async fn load_user(pool: &PgPool, org_id: Uuid, user_id: Uuid) -> Result<UserRow, AppError> {
    sqlx::query_as::<_, UserDbRow>(
        "SELECT p.id, si.external_id, p.handle, COALESCE(si.display_name, p.display_name),
                (m.status = 'active' AND p.status = 'active') AS active,
                p.created_at, p.updated_at
         FROM memberships m
         JOIN principals p ON p.id = m.principal_id
         JOIN scim_identities si ON si.org_id = m.org_id AND si.principal_id = p.id
         WHERE m.org_id = $1 AND p.id = $2 AND p.kind = 'human'",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .map(UserRow::from)
    .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "startIndex")]
    start_index: Option<u32>,
    count: Option<u32>,
    filter: Option<String>,
    #[serde(rename = "sortBy")]
    sort_by: Option<String>,
    #[serde(rename = "sortOrder")]
    sort_order: Option<String>,
}

fn user_sort_clause(sort_by: Option<&str>, sort_order: Option<&str>) -> Result<String, AppError> {
    let sort_expression = match sort_by.unwrap_or("userName").to_ascii_lowercase().as_str() {
        "username" => "lower(p.handle)",
        "externalid" => "si.external_id",
        "displayname" => "lower(si.display_name)",
        "active" => "(m.status = 'active' AND p.status = 'active')",
        "meta.created" => "p.created_at",
        "meta.lastmodified" => "p.updated_at",
        _ => {
            return Err(AppError::Validation(
                "unsupported SCIM sort attribute".into(),
            ));
        }
    };
    let direction = match sort_order
        .unwrap_or("ascending")
        .to_ascii_lowercase()
        .as_str()
    {
        "ascending" => "ASC",
        "descending" => "DESC",
        _ => return Err(AppError::Validation("invalid SCIM sortOrder".into())),
    };
    Ok(format!("{sort_expression} {direction}, p.id ASC"))
}

#[derive(Debug)]
enum UserFilter {
    UserName(String),
    ExternalId(String),
    DisplayName(String),
    Active(bool),
}

fn parse_user_filter(filter: &str) -> Result<UserFilter, AppError> {
    let (attribute, value) = filter
        .split_once(" eq ")
        .ok_or_else(|| AppError::Validation("SCIM filter must use attribute eq value".into()))?;
    let attribute = attribute.trim();
    let value = value.trim();
    match attribute {
        "userName" | "externalId" | "displayName" => {
            let value = value
                .strip_prefix('\"')
                .and_then(|v| v.strip_suffix('\"'))
                .ok_or_else(|| {
                    AppError::Validation("SCIM string filter value must be quoted".into())
                })?;
            if value.is_empty() || value.len() > 255 {
                return Err(AppError::Validation(
                    "SCIM filter value must be 1-255 characters".into(),
                ));
            }
            Ok(match attribute {
                "userName" => UserFilter::UserName(value.to_owned()),
                "externalId" => UserFilter::ExternalId(value.to_owned()),
                _ => UserFilter::DisplayName(value.to_owned()),
            })
        }
        "active" => match value {
            "true" => Ok(UserFilter::Active(true)),
            "false" => Ok(UserFilter::Active(false)),
            _ => Err(AppError::Validation(
                "SCIM active filter must be true or false".into(),
            )),
        },
        _ => Err(AppError::Validation(
            "unsupported SCIM filter attribute".into(),
        )),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse {
    schemas: Vec<&'static str>,
    total_results: i64,
    start_index: u32,
    items_per_page: usize,
    #[serde(rename = "Resources")]
    resources: Vec<ScimUser>,
}

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ListResponse>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let base = scim_base(&org);
    let start_index = query.start_index.unwrap_or(1).max(1);
    let count = query
        .count
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let filter = query.filter.as_deref().map(parse_user_filter).transpose()?;
    let order_by = user_sort_clause(query.sort_by.as_deref(), query.sort_order.as_deref())?;
    let filter_sql = match &filter {
        Some(UserFilter::UserName(_)) => " AND lower(p.handle) = lower($2)",
        Some(UserFilter::ExternalId(_)) => " AND si.external_id = $2",
        Some(UserFilter::DisplayName(_)) => " AND lower(si.display_name) = lower($2)",
        Some(UserFilter::Active(_)) => " AND (m.status = 'active' AND p.status = 'active') = $2",
        None => "",
    };
    let (offset_parameter, limit_parameter) = if filter.is_some() {
        ("$3", "$4")
    } else {
        ("$2", "$3")
    };
    let count_sql = format!(
        "SELECT COUNT(*) FROM memberships m
         JOIN principals p ON p.id = m.principal_id
         JOIN scim_identities si ON si.org_id = m.org_id AND si.principal_id = p.id
         WHERE m.org_id = $1 AND p.kind = 'human'{filter_sql}"
    );
    let mut count_query = sqlx::query_as::<_, (i64,)>(&count_sql).bind(ctx.org_id);
    count_query = match &filter {
        Some(
            UserFilter::UserName(value)
            | UserFilter::ExternalId(value)
            | UserFilter::DisplayName(value),
        ) => count_query.bind(value),
        Some(UserFilter::Active(value)) => count_query.bind(value),
        None => count_query,
    };
    let total_results = count_query.fetch_one(&state.pool).await?.0;

    let users_sql = format!(
        "SELECT p.id, si.external_id, p.handle, si.display_name,
                (m.status = 'active' AND p.status = 'active') AS active,
                p.created_at, p.updated_at
         FROM memberships m
         JOIN principals p ON p.id = m.principal_id
         JOIN scim_identities si ON si.org_id = m.org_id AND si.principal_id = p.id
         WHERE m.org_id = $1 AND p.kind = 'human'{filter_sql}
         ORDER BY {order_by}
         OFFSET {offset_parameter} LIMIT {limit_parameter}"
    );
    let mut users_query = sqlx::query_as::<_, UserDbRow>(&users_sql).bind(ctx.org_id);
    users_query = match &filter {
        Some(
            UserFilter::UserName(value)
            | UserFilter::ExternalId(value)
            | UserFilter::DisplayName(value),
        ) => users_query.bind(value),
        Some(UserFilter::Active(value)) => users_query.bind(value),
        None => users_query,
    };
    let rows = users_query
        .bind(i64::from(start_index - 1))
        .bind(i64::from(count))
        .fetch_all(&state.pool)
        .await?;
    let resources = rows
        .into_iter()
        .map(UserRow::from)
        .map(|row| to_scim_user(row, &base))
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(ListResponse {
        schemas: vec![LIST_SCHEMA],
        total_results,
        start_index,
        items_per_page: resources.len(),
        resources,
    }))
}

pub async fn get_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, user_id)): Path<(String, Uuid)>,
) -> Result<Json<ScimUser>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    Ok(Json(to_scim_user(
        load_user(&state.pool, ctx.org_id, user_id).await?,
        &scim_base(&org),
    )?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUser {
    pub schemas: Option<Vec<String>>,
    pub external_id: Option<String>,
    pub user_name: String,
    pub display_name: Option<String>,
    pub active: Option<bool>,
}

fn validate_user_name(value: &str) -> Result<(String, String), AppError> {
    let handle = value.trim();
    if handle.is_empty()
        || handle.len() > 64
        || !handle
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(AppError::Validation(
            "SCIM userName must be 1-64 ASCII letters, digits, '.', '_' or '-'".into(),
        ));
    }
    Ok((handle.to_string(), normalize_handle(handle)))
}

fn validate_external_id(value: Option<&str>) -> Result<String, AppError> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .ok_or_else(|| {
            AppError::Validation("SCIM externalId is required (1-255 characters)".into())
        })?;
    Ok(value.to_string())
}

pub async fn create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<CreateUser>,
) -> Result<Response, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let external_id = validate_external_id(body.external_id.as_deref())?;
    let (handle, normalized) = validate_user_name(&body.user_name)?;
    if body.active == Some(false) {
        return Err(AppError::Validation("new SCIM users must be active".into()).into());
    }
    let display_name = body
        .display_name
        .as_deref()
        .unwrap_or(&handle)
        .trim()
        .to_string();
    if display_name.is_empty() || display_name.len() > 200 {
        return Err(AppError::Validation("displayName must be 1-200 characters".into()).into());
    }
    let user_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let exists =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM principals WHERE handle_normalized = $1")
            .bind(&normalized)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_some() {
        return Err(AppError::Conflict("userName is already assigned to an account".into()).into());
    }
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', $2, $3, $4, 'active')",
    )
    .bind(user_id)
    .bind(&handle)
    .bind(&normalized)
    .bind(&handle)
    .execute(&mut *tx)
    .await
    .map_err(map_scim_user_creation_error)?;
    sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status)
         VALUES ($1, $2, $3, 'member', 'active')",
    )
    .bind(Uuid::new_v4())
    .bind(ctx.org_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO scim_identities (id, org_id, principal_id, external_id, display_name)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(ctx.org_id)
    .bind(user_id)
    .bind(&external_id)
    .bind(&display_name)
    .execute(&mut *tx)
    .await
    .map_err(map_scim_user_creation_error)?;
    record_scim_audit(&mut tx, ctx, "scim.user.create", user_id).await?;
    tx.commit().await?;
    let user = load_user(&state.pool, ctx.org_id, user_id).await?;
    let mut response = (
        StatusCode::CREATED,
        Json(to_scim_user(user, &scim_base(&org))?),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::LOCATION, scim_location(&org, user_id)?);
    Ok(response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceUser {
    pub external_id: Option<String>,
    pub user_name: String,
    pub display_name: Option<String>,
    pub active: Option<bool>,
}

pub async fn replace_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, user_id)): Path<(String, Uuid)>,
    Json(body): Json<ReplaceUser>,
) -> Result<Json<ScimUser>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let current = load_user(&state.pool, ctx.org_id, user_id).await?;
    let (_, normalized) = validate_user_name(&body.user_name)?;
    if normalized != normalize_handle(&current.handle) {
        return Err(AppError::Conflict(
            "SCIM userName cannot rename an existing MuseHub account".into(),
        )
        .into());
    }
    let external_id = validate_external_id(
        body.external_id
            .as_deref()
            .or(current.external_id.as_deref()),
    )?;
    let display_name = body
        .display_name
        .as_deref()
        .unwrap_or(&current.display_name)
        .trim();
    if display_name.is_empty() || display_name.len() > 200 {
        return Err(AppError::Validation("displayName must be 1-200 characters".into()).into());
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO scim_identities (id, org_id, principal_id, external_id, display_name)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (org_id, principal_id) DO UPDATE
         SET external_id = EXCLUDED.external_id, display_name = EXCLUDED.display_name",
    )
    .bind(Uuid::new_v4())
    .bind(ctx.org_id)
    .bind(user_id)
    .bind(external_id)
    .bind(display_name)
    .execute(&mut *tx)
    .await?;
    if let Some(active) = body.active {
        set_membership_active(&mut tx, ctx.org_id, user_id, active).await?;
    }
    record_scim_audit(&mut tx, ctx, "scim.user.replace", user_id).await?;
    tx.commit().await?;
    let user = load_user(&state.pool, ctx.org_id, user_id).await?;
    Ok(Json(to_scim_user(user, &scim_base(&org))?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchRequest {
    pub schemas: Option<Vec<String>>,
    pub operations: Vec<PatchOperation>,
}

#[derive(Deserialize)]
pub struct PatchOperation {
    pub op: String,
    pub path: Option<String>,
    pub value: Option<Value>,
}

pub async fn patch_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, user_id)): Path<(String, Uuid)>,
    Json(body): Json<PatchRequest>,
) -> Result<Json<ScimUser>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let current = load_user(&state.pool, ctx.org_id, user_id).await?;
    let mut active = current.active;
    let mut display_name = current.display_name.clone();
    for operation in body.operations {
        let op = operation.op.to_ascii_lowercase();
        if !matches!(op.as_str(), "replace" | "add" | "remove") {
            return Err(AppError::Validation(
                "SCIM PATCH op must be add, replace, or remove".into(),
            )
            .into());
        }
        if op == "remove" {
            let path = operation.path.as_deref().ok_or_else(|| {
                ScimApiError(AppError::Validation(
                    "SCIM PATCH remove requires a path".into(),
                ))
            })?;
            match path.to_ascii_lowercase().as_str() {
                "active" => active = true,
                "displayname" => display_name = current.handle.clone(),
                _ => return Err(AppError::Validation("unsupported SCIM PATCH path".into()).into()),
            }
            continue;
        }
        let value = operation.value.ok_or_else(|| {
            ScimApiError(AppError::Validation(
                "SCIM PATCH add/replace operation requires value".into(),
            ))
        })?;
        if let Some(path) = operation.path.as_deref() {
            match path.to_ascii_lowercase().as_str() {
                "active" => {
                    active = value
                        .as_bool()
                        .ok_or_else(|| AppError::Validation("active must be boolean".into()))?
                }
                "displayname" => {
                    display_name = value
                        .as_str()
                        .ok_or_else(|| AppError::Validation("displayName must be a string".into()))?
                        .to_string()
                }
                _ => return Err(AppError::Validation("unsupported SCIM PATCH path".into()).into()),
            }
        } else {
            let fields = value.as_object().ok_or_else(|| {
                AppError::Validation(
                    "SCIM PATCH value must be an object when path is omitted".into(),
                )
            })?;
            for (key, value) in fields {
                match key.to_ascii_lowercase().as_str() {
                    "active" => {
                        active = value
                            .as_bool()
                            .ok_or_else(|| AppError::Validation("active must be boolean".into()))?
                    }
                    "displayname" => {
                        display_name = value
                            .as_str()
                            .ok_or_else(|| {
                                AppError::Validation("displayName must be a string".into())
                            })?
                            .to_string()
                    }
                    _ => {
                        return Err(
                            AppError::Validation("unsupported SCIM PATCH attribute".into()).into(),
                        )
                    }
                }
            }
        }
    }
    if display_name.trim().is_empty() || display_name.len() > 200 {
        return Err(AppError::Validation("displayName must be 1-200 characters".into()).into());
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE scim_identities SET display_name = $3
         WHERE org_id = $1 AND principal_id = $2",
    )
    .bind(ctx.org_id)
    .bind(user_id)
    .bind(display_name.trim())
    .execute(&mut *tx)
    .await?;
    set_membership_active(&mut tx, ctx.org_id, user_id, active).await?;
    record_scim_audit(&mut tx, ctx, "scim.user.patch", user_id).await?;
    tx.commit().await?;
    Ok(Json(to_scim_user(
        load_user(&state.pool, ctx.org_id, user_id).await?,
        &scim_base(&org),
    )?))
}

pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, user_id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    load_user(&state.pool, ctx.org_id, user_id).await?;
    let mut tx = state.pool.begin().await?;
    set_membership_active(&mut tx, ctx.org_id, user_id, false).await?;
    record_scim_audit(&mut tx, ctx, "scim.user.delete", user_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimGroup {
    pub schemas: Vec<&'static str>,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub display_name: String,
    pub members: Vec<ScimGroupMember>,
    pub meta: ScimMeta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimGroupMember {
    pub value: String,
    pub display: String,
    #[serde(rename = "type")]
    pub member_type: &'static str,
    #[serde(rename = "$ref")]
    pub reference: String,
}

type GroupDbRow = (Uuid, Option<String>, String, OffsetDateTime, OffsetDateTime);

const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";

fn validate_group_display_name(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 200 {
        return Err(AppError::Validation(
            "Group displayName must be 1-200 characters".into(),
        ));
    }
    Ok(value.to_string())
}

fn parse_group_member_ids(members: Option<Vec<GroupMemberInput>>) -> Result<Vec<Uuid>, AppError> {
    let mut ids = Vec::new();
    for member in members.unwrap_or_default() {
        if member
            .member_type
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case("User"))
        {
            return Err(AppError::Validation(
                "SCIM Group members must reference Users".into(),
            ));
        }
        let id = Uuid::parse_str(&member.value).map_err(|_| {
            AppError::Validation("SCIM Group member value must be a User UUID".into())
        })?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberInput {
    value: String,
    #[serde(rename = "type")]
    member_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroup {
    external_id: Option<String>,
    display_name: String,
    members: Option<Vec<GroupMemberInput>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceGroup {
    external_id: Option<String>,
    display_name: String,
    members: Option<Vec<GroupMemberInput>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupListResponse {
    schemas: Vec<&'static str>,
    total_results: i64,
    start_index: u32,
    items_per_page: usize,
    #[serde(rename = "Resources")]
    resources: Vec<ScimGroup>,
}

fn group_sort_clause(sort_by: Option<&str>, sort_order: Option<&str>) -> Result<String, AppError> {
    let expression = match sort_by
        .unwrap_or("displayName")
        .to_ascii_lowercase()
        .as_str()
    {
        "displayname" => "lower(g.display_name)",
        "externalid" => "g.external_id",
        "meta.created" => "g.created_at",
        "meta.lastmodified" => "g.updated_at",
        _ => {
            return Err(AppError::Validation(
                "unsupported SCIM Group sort attribute".into(),
            ))
        }
    };
    let direction = match sort_order
        .unwrap_or("ascending")
        .to_ascii_lowercase()
        .as_str()
    {
        "ascending" => "ASC",
        "descending" => "DESC",
        _ => return Err(AppError::Validation("invalid SCIM sortOrder".into())),
    };
    Ok(format!("{expression} {direction}, g.id ASC"))
}

fn parse_group_filter(filter: Option<&str>) -> Result<Option<(bool, String)>, AppError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    let (attribute, value) = filter
        .split_once(" eq ")
        .ok_or_else(|| AppError::Validation("SCIM filter must use attribute eq value".into()))?;
    let external_id = match attribute.trim().to_ascii_lowercase().as_str() {
        "displayname" => false,
        "externalid" => true,
        _ => {
            return Err(AppError::Validation(
                "unsupported SCIM Group filter attribute".into(),
            ))
        }
    };
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err(AppError::Validation(
            "SCIM string filter value must be quoted".into(),
        ));
    }
    let value = value[1..value.len() - 1].to_string();
    if value.is_empty() || value.len() > 255 {
        return Err(AppError::Validation(
            "SCIM filter value must be 1-255 characters".into(),
        ));
    }
    Ok(Some((external_id, value)))
}

async fn load_group_members(
    pool: &PgPool,
    org_id: Uuid,
    group_ids: &[Uuid],
    org: &str,
) -> Result<std::collections::HashMap<Uuid, Vec<ScimGroupMember>>, AppError> {
    if group_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        "SELECT gm.group_id, p.id, si.display_name
         FROM scim_group_members gm
         JOIN principals p ON p.id = gm.principal_id
         JOIN scim_identities si ON si.org_id = gm.org_id AND si.principal_id = p.id
         WHERE gm.org_id = $1 AND gm.group_id = ANY($2)
         ORDER BY lower(p.handle), p.id",
    )
    .bind(org_id)
    .bind(group_ids)
    .fetch_all(pool)
    .await?;
    let base = format!("/api/v1/scim/v2/organizations/{org}/Users");
    let mut members = std::collections::HashMap::<Uuid, Vec<ScimGroupMember>>::new();
    for (group_id, user_id, display_name) in rows {
        let value = user_id.to_string();
        members.entry(group_id).or_default().push(ScimGroupMember {
            reference: format!("{base}/{value}"),
            value,
            display: display_name,
            member_type: "User",
        });
    }
    Ok(members)
}

fn to_scim_group(
    row: GroupDbRow,
    members: Vec<ScimGroupMember>,
    org: &str,
) -> Result<ScimGroup, AppError> {
    let id = row.0.to_string();
    let base = scim_groups_base(org);
    Ok(ScimGroup {
        schemas: vec![GROUP_SCHEMA],
        id,
        external_id: row.1,
        display_name: row.2,
        members,
        meta: ScimMeta {
            resource_type: "Group",
            created: format_scim_timestamp(row.3)?,
            last_modified: format_scim_timestamp(row.4)?,
            location: format!("{base}/{}", row.0),
        },
    })
}

async fn load_group(
    pool: &PgPool,
    org_id: Uuid,
    group_id: Uuid,
    org: &str,
) -> Result<ScimGroup, AppError> {
    let row = sqlx::query_as::<_, GroupDbRow>(
        "SELECT id, external_id, display_name, created_at, updated_at
         FROM scim_groups WHERE org_id = $1 AND id = $2",
    )
    .bind(org_id)
    .bind(group_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let members = load_group_members(pool, org_id, &[group_id], org).await?;
    to_scim_group(
        row,
        members.get(&group_id).cloned().unwrap_or_default(),
        org,
    )
}

async fn replace_group_members(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
    group_id: Uuid,
    user_ids: &[Uuid],
) -> Result<(), AppError> {
    let found = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(DISTINCT principal_id) FROM scim_identities
         WHERE org_id = $1 AND principal_id = ANY($2)",
    )
    .bind(org_id)
    .bind(user_ids)
    .fetch_one(&mut **tx)
    .await?
    .0;
    if found != user_ids.len() as i64 {
        return Err(AppError::Validation(
            "SCIM Group members must be provisioned Users in this organization".into(),
        ));
    }
    sqlx::query("DELETE FROM scim_group_members WHERE org_id = $1 AND group_id = $2")
        .bind(org_id)
        .bind(group_id)
        .execute(&mut **tx)
        .await?;
    if !user_ids.is_empty() {
        sqlx::query(
            "INSERT INTO scim_group_members (org_id, group_id, principal_id)
             SELECT $1, $2, member_id FROM UNNEST($3::uuid[]) AS members(member_id)",
        )
        .bind(org_id)
        .bind(group_id)
        .bind(user_ids)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn list_groups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<GroupListResponse>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let start_index = query.start_index.unwrap_or(1).max(1);
    let count = query
        .count
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let filter = parse_group_filter(query.filter.as_deref())?;
    let order_by = group_sort_clause(query.sort_by.as_deref(), query.sort_order.as_deref())?;
    let filter_external_id = filter.as_ref().map(|(external_id, _)| *external_id);
    let filter_value = filter.as_ref().map(|(_, value)| value.as_str());
    let count_sql = "SELECT COUNT(*) FROM scim_groups g WHERE g.org_id = $1
         AND ($2::boolean IS NULL OR ($2 AND g.external_id = $3)
              OR (NOT $2 AND lower(g.display_name) = lower($3)))";
    let total_results = sqlx::query_as::<_, (i64,)>(count_sql)
        .bind(ctx.org_id)
        .bind(filter_external_id)
        .bind(filter_value)
        .fetch_one(&state.pool)
        .await?
        .0;
    let groups_sql = format!(
        "SELECT g.id, g.external_id, g.display_name, g.created_at, g.updated_at
         FROM scim_groups g WHERE g.org_id = $1
           AND ($2::boolean IS NULL OR ($2 AND g.external_id = $3)
                OR (NOT $2 AND lower(g.display_name) = lower($3)))
         ORDER BY {order_by} OFFSET $4 LIMIT $5"
    );
    let offset = i64::from(start_index - 1);
    let rows = sqlx::query_as::<_, GroupDbRow>(&groups_sql)
        .bind(ctx.org_id)
        .bind(filter_external_id)
        .bind(filter_value)
        .bind(offset)
        .bind(i64::from(count))
        .fetch_all(&state.pool)
        .await?;
    let ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
    let members = load_group_members(&state.pool, ctx.org_id, &ids, &org).await?;
    let resources = rows
        .into_iter()
        .map(|row| {
            let members = members.get(&row.0).cloned().unwrap_or_default();
            to_scim_group(row, members, &org)
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(GroupListResponse {
        schemas: vec![LIST_SCHEMA],
        total_results,
        start_index,
        items_per_page: resources.len(),
        resources,
    }))
}

pub async fn get_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, group_id)): Path<(String, Uuid)>,
) -> Result<Json<ScimGroup>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    Ok(Json(
        load_group(&state.pool, ctx.org_id, group_id, &org).await?,
    ))
}

pub async fn create_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<CreateGroup>,
) -> Result<Response, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let display_name = validate_group_display_name(&body.display_name)?;
    let external_id = body
        .external_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if external_id.is_some_and(|v| v.len() > 255) {
        return Err(
            AppError::Validation("Group externalId must be at most 255 characters".into()).into(),
        );
    }
    let member_ids = parse_group_member_ids(body.members)?;
    let group_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO scim_groups (id, org_id, external_id, display_name) VALUES ($1,$2,$3,$4)",
    )
    .bind(group_id)
    .bind(ctx.org_id)
    .bind(external_id)
    .bind(display_name)
    .execute(&mut *tx)
    .await
    .map_err(map_scim_group_write_error)?;
    replace_group_members(&mut tx, ctx.org_id, group_id, &member_ids).await?;
    crate::audit::record_in_tx(
        &mut tx,
        Some(ctx.org_id),
        None,
        ctx.actor_id,
        Some(ctx.actor_id),
        "scim.group.create",
        "scim_group",
        group_id,
        None,
        "allow",
        serde_json::json!({"scim_token_id": ctx.token_id}),
    )
    .await?;
    tx.commit().await?;
    let group = load_group(&state.pool, ctx.org_id, group_id, &org).await?;
    let mut response = (StatusCode::CREATED, Json(group)).into_response();
    response.headers_mut().insert(
        header::LOCATION,
        axum::http::HeaderValue::from_str(&format!("{}/{group_id}", scim_groups_base(&org)))
            .map_err(|_| AppError::Other("invalid SCIM Group location".into()))?,
    );
    Ok(response)
}

pub async fn replace_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, group_id)): Path<(String, Uuid)>,
    Json(body): Json<ReplaceGroup>,
) -> Result<Json<ScimGroup>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let display_name = validate_group_display_name(&body.display_name)?;
    let external_id = body
        .external_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if external_id.is_some_and(|v| v.len() > 255) {
        return Err(
            AppError::Validation("Group externalId must be at most 255 characters".into()).into(),
        );
    }
    let member_ids = parse_group_member_ids(body.members)?;
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE scim_groups SET external_id = $3, display_name = $4 WHERE org_id = $1 AND id = $2",
    )
    .bind(ctx.org_id)
    .bind(group_id)
    .bind(external_id)
    .bind(display_name)
    .execute(&mut *tx)
    .await
    .map_err(map_scim_group_write_error)?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound.into());
    }
    replace_group_members(&mut tx, ctx.org_id, group_id, &member_ids).await?;
    crate::audit::record_in_tx(
        &mut tx,
        Some(ctx.org_id),
        None,
        ctx.actor_id,
        Some(ctx.actor_id),
        "scim.group.replace",
        "scim_group",
        group_id,
        None,
        "allow",
        serde_json::json!({"scim_token_id": ctx.token_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        load_group(&state.pool, ctx.org_id, group_id, &org).await?,
    ))
}

pub async fn patch_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, group_id)): Path<(String, Uuid)>,
    Json(body): Json<PatchRequest>,
) -> Result<Json<ScimGroup>, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let current = load_group(&state.pool, ctx.org_id, group_id, &org).await?;
    let mut display_name = current.display_name;
    let mut member_ids = current
        .members
        .iter()
        .map(|member| Uuid::parse_str(&member.value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::Other("stored SCIM Group member ID is invalid".into()))?;
    if body.operations.is_empty() {
        return Err(
            AppError::Validation("SCIM PATCH requires at least one operation".into()).into(),
        );
    }
    for operation in body.operations {
        let op = operation.op.to_ascii_lowercase();
        if !matches!(op.as_str(), "add" | "replace" | "remove") {
            return Err(AppError::Validation(
                "SCIM PATCH op must be add, replace, or remove".into(),
            )
            .into());
        }
        let path = operation.path.as_deref().ok_or_else(|| {
            ScimApiError(AppError::Validation(
                "SCIM Group PATCH requires a path".into(),
            ))
        })?;
        if path.eq_ignore_ascii_case("displayName") {
            if op == "remove" {
                return Err(AppError::Validation("Group displayName is required".into()).into());
            }
            display_name = operation
                .value
                .as_ref()
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::Validation("displayName must be a string".into()))?
                .to_string();
            continue;
        }
        if path.eq_ignore_ascii_case("members") {
            if op == "remove" {
                member_ids.clear();
                continue;
            }
            let value = operation.value.ok_or_else(|| {
                ScimApiError(AppError::Validation(
                    "members operation requires a value".into(),
                ))
            })?;
            let members = match value {
                Value::Array(values) => values
                    .into_iter()
                    .map(serde_json::from_value::<GroupMemberInput>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        AppError::Validation("members must contain User references".into())
                    })?,
                object @ Value::Object(_) => {
                    vec![
                        serde_json::from_value::<GroupMemberInput>(object).map_err(|_| {
                            AppError::Validation("members must contain User references".into())
                        })?,
                    ]
                }
                _ => {
                    return Err(AppError::Validation(
                        "members must be an array or User reference".into(),
                    )
                    .into())
                }
            };
            let additions = parse_group_member_ids(Some(members))?;
            if op == "replace" {
                member_ids = additions;
            } else {
                for id in additions {
                    if !member_ids.contains(&id) {
                        member_ids.push(id);
                    }
                }
            }
            continue;
        }
        if op == "remove" {
            if let Some(id_value) = path
                .strip_prefix("members[value eq ")
                .and_then(|p| p.strip_suffix(']'))
            {
                let id_value = id_value.trim().trim_matches('"');
                let id = Uuid::parse_str(id_value).map_err(|_| {
                    AppError::Validation("members filter must contain a User UUID".into())
                })?;
                let previous_len = member_ids.len();
                member_ids.retain(|member_id| *member_id != id);
                if member_ids.len() == previous_len {
                    return Err(AppError::NotFound.into());
                }
                continue;
            }
        }
        return Err(AppError::Validation("unsupported SCIM Group PATCH path".into()).into());
    }
    display_name = validate_group_display_name(&display_name)?;
    let mut tx = state.pool.begin().await?;
    let updated =
        sqlx::query("UPDATE scim_groups SET display_name = $3 WHERE org_id = $1 AND id = $2")
            .bind(ctx.org_id)
            .bind(group_id)
            .bind(display_name)
            .execute(&mut *tx)
            .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound.into());
    }
    replace_group_members(&mut tx, ctx.org_id, group_id, &member_ids).await?;
    crate::audit::record_in_tx(
        &mut tx,
        Some(ctx.org_id),
        None,
        ctx.actor_id,
        Some(ctx.actor_id),
        "scim.group.patch",
        "scim_group",
        group_id,
        None,
        "allow",
        serde_json::json!({"scim_token_id": ctx.token_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        load_group(&state.pool, ctx.org_id, group_id, &org).await?,
    ))
}

pub async fn delete_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, group_id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ScimApiError> {
    let ctx = authenticate(&state.pool, &org, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let deleted = sqlx::query("DELETE FROM scim_groups WHERE org_id = $1 AND id = $2")
        .bind(ctx.org_id)
        .bind(group_id)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound.into());
    }
    crate::audit::record_in_tx(
        &mut tx,
        Some(ctx.org_id),
        None,
        ctx.actor_id,
        Some(ctx.actor_id),
        "scim.group.delete",
        "scim_group",
        group_id,
        None,
        "allow",
        serde_json::json!({"scim_token_id": ctx.token_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub const MAX_BULK_OPERATIONS: usize = 100;
pub const MAX_BULK_PAYLOAD_BYTES: usize = 1_048_576;
const BULK_REQUEST_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:BulkRequest";
const BULK_RESPONSE_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:BulkResponse";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScimBulkRequest {
    schemas: Vec<String>,
    fail_on_errors: Option<usize>,
    #[serde(rename = "Operations")]
    operations: Vec<ScimBulkOperation>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScimBulkOperation {
    method: String,
    bulk_id: Option<String>,
    path: String,
    data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScimBulkResponse {
    schemas: [&'static str; 1],
    #[serde(rename = "Operations")]
    operations: Vec<ScimBulkOperationResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScimBulkOperationResponse {
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bulk_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<Value>,
}

#[derive(Debug)]
enum ScimBulkPath {
    Users,
    Groups,
    User(Uuid),
    Group(Uuid),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScimBulkResourceType {
    User,
    Group,
}

#[derive(Clone, Copy)]
struct ScimBulkResourceRef {
    resource_type: ScimBulkResourceType,
    id: Uuid,
}

fn resolve_bulk_reference(
    reference: &str,
    expected_type: ScimBulkResourceType,
    created: &HashMap<String, ScimBulkResourceRef>,
) -> Result<Uuid, AppError> {
    let bulk_id = reference
        .strip_prefix("bulkId:")
        .filter(|bulk_id| !bulk_id.is_empty())
        .ok_or_else(|| AppError::Validation("invalid SCIM Bulk reference".into()))?;
    let resolved = created
        .get(bulk_id)
        .ok_or_else(|| AppError::Validation("unknown or forward SCIM Bulk reference".into()))?;
    if resolved.resource_type != expected_type {
        return Err(AppError::Validation(
            "SCIM Bulk reference has the wrong resource type".into(),
        ));
    }
    Ok(resolved.id)
}

fn resolve_bulk_value_references(
    value: &mut Value,
    created: &HashMap<String, ScimBulkResourceRef>,
) -> Result<(), AppError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if key == "value" {
                    if let Some(reference) =
                        child.as_str().filter(|value| value.starts_with("bulkId:"))
                    {
                        let id =
                            resolve_bulk_reference(reference, ScimBulkResourceType::User, created)?;
                        *child = Value::String(id.to_string());
                        continue;
                    }
                }
                resolve_bulk_value_references(child, created)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                resolve_bulk_value_references(item, created)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn resolve_bulk_operation(
    operation: &ScimBulkOperation,
    created: &HashMap<String, ScimBulkResourceRef>,
) -> Result<ScimBulkOperation, AppError> {
    let mut resolved = operation.clone();
    let path = resolved.path.strip_prefix('/').unwrap_or(&resolved.path);
    let mut parts = path.split('/');
    let resource = parts.next().unwrap_or_default();
    let id = parts.next();
    if parts.next().is_some() {
        return Err(AppError::Validation("unsupported SCIM Bulk path".into()));
    }
    if let Some(reference) = id.filter(|id| id.starts_with("bulkId:")) {
        let expected_type = match resource {
            "Users" => ScimBulkResourceType::User,
            "Groups" => ScimBulkResourceType::Group,
            _ => return Err(AppError::Validation("unsupported SCIM Bulk path".into())),
        };
        let id = resolve_bulk_reference(reference, expected_type, created)?;
        resolved.path = format!("/{resource}/{id}");
    }
    if let Some(data) = &mut resolved.data {
        resolve_bulk_value_references(data, created)?;
    }
    Ok(resolved)
}

fn parse_bulk_path(path: &str) -> Result<ScimBulkPath, AppError> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let mut parts = path.split('/');
    let resource = parts.next().unwrap_or_default();
    let id = parts.next();
    if parts.next().is_some() {
        return Err(AppError::Validation("unsupported SCIM Bulk path".into()));
    }
    match (resource, id) {
        ("Users", None) => Ok(ScimBulkPath::Users),
        ("Groups", None) => Ok(ScimBulkPath::Groups),
        ("Users", Some(id)) => Uuid::parse_str(id)
            .map(ScimBulkPath::User)
            .map_err(|_| AppError::Validation("SCIM Bulk User ID must be a UUID".into())),
        ("Groups", Some(id)) => Uuid::parse_str(id)
            .map(ScimBulkPath::Group)
            .map_err(|_| AppError::Validation("SCIM Bulk Group ID must be a UUID".into())),
        _ => Err(AppError::Validation("unsupported SCIM Bulk path".into())),
    }
}

fn bulk_error_response(error: AppError) -> Response {
    ScimApiError(error).into_response()
}

async fn execute_bulk_operation(
    state: Arc<AppState>,
    headers: HeaderMap,
    org: String,
    operation: &ScimBulkOperation,
) -> Response {
    let path = match parse_bulk_path(&operation.path) {
        Ok(path) => path,
        Err(error) => return bulk_error_response(error),
    };
    let method = operation.method.to_ascii_uppercase();
    match (method.as_str(), path) {
        ("POST", ScimBulkPath::Users) => {
            let Some(data) = operation.data.clone() else {
                return bulk_error_response(AppError::Validation(
                    "SCIM Bulk POST requires data".into(),
                ));
            };
            let body = match serde_json::from_value::<CreateUser>(data) {
                Ok(body) => body,
                Err(_) => {
                    return bulk_error_response(AppError::Validation(
                        "invalid SCIM Bulk User data".into(),
                    ));
                }
            };
            match create_user(State(state), headers, Path(org), Json(body)).await {
                Ok(response) => response,
                Err(error) => error.into_response(),
            }
        }
        ("POST", ScimBulkPath::Groups) => {
            let Some(data) = operation.data.clone() else {
                return bulk_error_response(AppError::Validation(
                    "SCIM Bulk POST requires data".into(),
                ));
            };
            let body = match serde_json::from_value::<CreateGroup>(data) {
                Ok(body) => body,
                Err(_) => {
                    return bulk_error_response(AppError::Validation(
                        "invalid SCIM Bulk Group data".into(),
                    ));
                }
            };
            match create_group(State(state), headers, Path(org), Json(body)).await {
                Ok(response) => response,
                Err(error) => error.into_response(),
            }
        }
        ("PUT", ScimBulkPath::User(user_id)) => {
            let Some(data) = operation.data.clone() else {
                return bulk_error_response(AppError::Validation(
                    "SCIM Bulk PUT requires data".into(),
                ));
            };
            let body = match serde_json::from_value::<ReplaceUser>(data) {
                Ok(body) => body,
                Err(_) => {
                    return bulk_error_response(AppError::Validation(
                        "invalid SCIM Bulk User data".into(),
                    ));
                }
            };
            match replace_user(State(state), headers, Path((org, user_id)), Json(body)).await {
                Ok(response) => response.into_response(),
                Err(error) => error.into_response(),
            }
        }
        ("PATCH", ScimBulkPath::User(user_id)) => {
            let Some(data) = operation.data.clone() else {
                return bulk_error_response(AppError::Validation(
                    "SCIM Bulk PATCH requires data".into(),
                ));
            };
            let body = match serde_json::from_value::<PatchRequest>(data) {
                Ok(body) => body,
                Err(_) => {
                    return bulk_error_response(AppError::Validation(
                        "invalid SCIM Bulk User PATCH data".into(),
                    ));
                }
            };
            match patch_user(State(state), headers, Path((org, user_id)), Json(body)).await {
                Ok(response) => response.into_response(),
                Err(error) => error.into_response(),
            }
        }
        ("DELETE", ScimBulkPath::User(user_id)) => {
            match delete_user(State(state), headers, Path((org, user_id))).await {
                Ok(status) => status.into_response(),
                Err(error) => error.into_response(),
            }
        }
        ("PUT", ScimBulkPath::Group(group_id)) => {
            let Some(data) = operation.data.clone() else {
                return bulk_error_response(AppError::Validation(
                    "SCIM Bulk PUT requires data".into(),
                ));
            };
            let body = match serde_json::from_value::<ReplaceGroup>(data) {
                Ok(body) => body,
                Err(_) => {
                    return bulk_error_response(AppError::Validation(
                        "invalid SCIM Bulk Group data".into(),
                    ));
                }
            };
            match replace_group(State(state), headers, Path((org, group_id)), Json(body)).await {
                Ok(response) => response.into_response(),
                Err(error) => error.into_response(),
            }
        }
        ("PATCH", ScimBulkPath::Group(group_id)) => {
            let Some(data) = operation.data.clone() else {
                return bulk_error_response(AppError::Validation(
                    "SCIM Bulk PATCH requires data".into(),
                ));
            };
            let body = match serde_json::from_value::<PatchRequest>(data) {
                Ok(body) => body,
                Err(_) => {
                    return bulk_error_response(AppError::Validation(
                        "invalid SCIM Bulk Group PATCH data".into(),
                    ));
                }
            };
            match patch_group(State(state), headers, Path((org, group_id)), Json(body)).await {
                Ok(response) => response.into_response(),
                Err(error) => error.into_response(),
            }
        }
        ("DELETE", ScimBulkPath::Group(group_id)) => {
            match delete_group(State(state), headers, Path((org, group_id))).await {
                Ok(status) => status.into_response(),
                Err(error) => error.into_response(),
            }
        }
        _ => bulk_error_response(AppError::Validation(
            "unsupported SCIM Bulk method and resource combination".into(),
        )),
    }
}

pub(crate) async fn bulk(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(request): Json<ScimBulkRequest>,
) -> Result<Response, ScimApiError> {
    authenticate(&state.pool, &org, &headers).await?;
    if !request
        .schemas
        .iter()
        .any(|schema| schema == BULK_REQUEST_SCHEMA)
    {
        return Err(AppError::Validation("invalid SCIM Bulk request schema".into()).into());
    }
    if request.operations.is_empty() || request.operations.len() > MAX_BULK_OPERATIONS {
        return Err(AppError::Validation(format!(
            "SCIM Bulk operations must contain 1-{MAX_BULK_OPERATIONS} items"
        ))
        .into());
    }

    let mut operations = Vec::with_capacity(request.operations.len());
    let mut error_count = 0;
    let mut created_resources = HashMap::new();
    let mut declared_bulk_ids = std::collections::HashSet::new();
    for operation in &request.operations {
        if request
            .fail_on_errors
            .is_some_and(|limit| limit > 0 && error_count >= limit)
        {
            break;
        }
        let method = operation.method.to_ascii_uppercase();
        let duplicate_bulk_id = operation
            .bulk_id
            .as_ref()
            .is_some_and(|bulk_id| !declared_bulk_ids.insert(bulk_id.clone()));
        let response = if duplicate_bulk_id {
            bulk_error_response(AppError::Validation(
                "SCIM Bulk bulkId values must be unique".into(),
            ))
        } else if operation.bulk_id.as_ref().is_some_and(String::is_empty) {
            bulk_error_response(AppError::Validation(
                "SCIM Bulk bulkId must not be empty".into(),
            ))
        } else if operation.bulk_id.is_some() && method != "POST" {
            bulk_error_response(AppError::Validation(
                "SCIM Bulk bulkId is only valid for POST operations".into(),
            ))
        } else {
            match resolve_bulk_operation(operation, &created_resources) {
                Ok(resolved) => {
                    execute_bulk_operation(state.clone(), headers.clone(), org.clone(), &resolved)
                        .await
                }
                Err(error) => bulk_error_response(error),
            }
        };
        let status = response.status();
        if !status.is_success() {
            error_count += 1;
        }
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let response_body = axum::body::to_bytes(response.into_body(), MAX_BULK_PAYLOAD_BYTES)
            .await
            .ok()
            .filter(|bytes| !bytes.is_empty())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if status.is_success() && method == "POST" {
            if let (Some(bulk_id), Some(resource_id)) = (
                operation.bulk_id.as_ref(),
                response_body
                    .as_ref()
                    .and_then(|body| body.get("id"))
                    .and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok()),
            ) {
                let resource_type = match operation.path.trim_start_matches('/').split('/').next() {
                    Some("Users") => Some(ScimBulkResourceType::User),
                    Some("Groups") => Some(ScimBulkResourceType::Group),
                    _ => None,
                };
                if let Some(resource_type) = resource_type {
                    created_resources.insert(
                        bulk_id.clone(),
                        ScimBulkResourceRef {
                            resource_type,
                            id: resource_id,
                        },
                    );
                }
            }
        }
        operations.push(ScimBulkOperationResponse {
            method,
            bulk_id: operation.bulk_id.clone(),
            location,
            status: status.as_u16().to_string(),
            response: response_body,
        });
    }
    let mut response = Json(ScimBulkResponse {
        schemas: [BULK_RESPONSE_SCHEMA],
        operations,
    })
    .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/scim+json"),
    );
    Ok(response)
}

async fn set_membership_active(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
    user_id: Uuid,
    active: bool,
) -> Result<(), AppError> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT role, status FROM memberships WHERE org_id = $1 AND principal_id = $2 FOR UPDATE",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if row.0 == "owner" {
        return Err(AppError::Forbidden);
    }
    if !active && row.1 == "active" {
        sqlx::query(
            "UPDATE memberships SET status = 'inactive' WHERE org_id = $1 AND principal_id = $2",
        )
        .bind(org_id)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    } else if active && row.1 != "active" {
        sqlx::query(
            "UPDATE memberships SET status = 'active' WHERE org_id = $1 AND principal_id = $2",
        )
        .bind(org_id)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn record_scim_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ctx: ScimContext,
    action: &str,
    user_id: Uuid,
) -> Result<(), AppError> {
    crate::audit::record_in_tx(
        tx,
        Some(ctx.org_id),
        None,
        ctx.actor_id,
        Some(ctx.actor_id),
        action,
        "principal",
        user_id,
        None,
        "allow",
        serde_json::json!({ "scim_token_id": ctx.token_id }),
    )
    .await?;
    Ok(())
}

pub async fn service_provider_config(Path(_org): Path<String>) -> Json<Value> {
    Json(serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
        "patch": { "supported": true },
        "bulk": {
            "supported": true,
            "maxOperations": MAX_BULK_OPERATIONS,
            "maxPayloadSize": MAX_BULK_PAYLOAD_BYTES
        },
        "filter": { "supported": true, "maxResults": MAX_PAGE_SIZE },
        "changePassword": { "supported": false },
        "sort": { "supported": true },
        "etag": { "supported": false },
        "authenticationSchemes": [{
            "type": "oauthbearertoken",
            "name": "Bearer Token",
            "description": "Organization-scoped SCIM bearer token",
            "primary": true
        }]
    }))
}

pub async fn schemas(Path(_org): Path<String>) -> Json<Value> {
    Json(serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
        "totalResults": 2,
        "startIndex": 1,
        "itemsPerPage": 2,
        "Resources": [{
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Schema"],
            "id": USER_SCHEMA,
            "name": "User",
            "description": "Organization user",
            "attributes": [
                { "name": "userName", "type": "string", "multiValued": false, "required": true, "caseExact": false, "mutability": "readWrite", "returned": "default", "uniqueness": "global" },
                { "name": "externalId", "type": "string", "multiValued": false, "required": false, "caseExact": true, "mutability": "readWrite", "returned": "default", "uniqueness": "server" },
                { "name": "displayName", "type": "string", "multiValued": false, "required": false, "caseExact": false, "mutability": "readWrite", "returned": "default" },
                { "name": "active", "type": "boolean", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default" }
            ]
        }, {
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Schema"],
            "id": GROUP_SCHEMA,
            "name": "Group",
            "description": "Organization group directory entry; membership does not grant application permissions",
            "attributes": [
                { "name": "externalId", "type": "string", "multiValued": false, "required": false, "caseExact": true, "mutability": "readWrite", "returned": "default", "uniqueness": "server" },
                { "name": "displayName", "type": "string", "multiValued": false, "required": true, "caseExact": false, "mutability": "readWrite", "returned": "default" },
                { "name": "members", "type": "complex", "multiValued": true, "required": false, "mutability": "readWrite", "returned": "default", "subAttributes": [
                    { "name": "value", "type": "string", "multiValued": false, "required": true },
                    { "name": "type", "type": "string", "multiValued": false, "required": false },
                    { "name": "$ref", "type": "reference", "referenceTypes": ["User"], "multiValued": false, "required": false },
                    { "name": "display", "type": "string", "multiValued": false, "required": false }
                ]}
            ]
        }]
    }))
}

pub async fn resource_types(Path(org): Path<String>) -> Json<Value> {
    Json(serde_json::json!({
        "schemas": [LIST_SCHEMA],
        "totalResults": 2,
        "startIndex": 1,
        "itemsPerPage": 2,
        "Resources": [{
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],
            "id": "User",
            "name": "User",
            "endpoint": format!("/api/v1/scim/v2/organizations/{org}/Users"),
            "schema": USER_SCHEMA
        }, {
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],
            "id": "Group",
            "name": "Group",
            "endpoint": format!("/api/v1/scim/v2/organizations/{org}/Groups"),
            "schema": GROUP_SCHEMA
        }]
    }))
}

fn scim_base(org: &str) -> String {
    format!("/api/v1/scim/v2/organizations/{org}/Users")
}

fn scim_groups_base(org: &str) -> String {
    format!("/api/v1/scim/v2/organizations/{org}/Groups")
}

fn scim_location(org: &str, user_id: Uuid) -> Result<axum::http::HeaderValue, AppError> {
    axum::http::HeaderValue::from_str(&format!("{}/{user_id}", scim_base(org)))
        .map_err(|_| AppError::Other("invalid SCIM resource location".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[test]
    fn scim_bulk_accepts_only_supported_resource_paths() {
        let user_id = Uuid::new_v4();
        assert!(matches!(parse_bulk_path("/Users"), Ok(ScimBulkPath::Users)));
        assert!(matches!(
            parse_bulk_path("Groups"),
            Ok(ScimBulkPath::Groups)
        ));
        assert!(matches!(
            parse_bulk_path(&format!("/Users/{user_id}")),
            Ok(ScimBulkPath::User(id)) if id == user_id
        ));
        assert!(parse_bulk_path("/Users/not-a-uuid").is_err());
        assert!(parse_bulk_path("/Users/id/extra").is_err());
        assert!(parse_bulk_path("/Organizations/demo/Users").is_err());
    }

    #[test]
    fn scim_bulk_resolves_prior_user_references_in_paths_and_group_members() {
        let user_id = Uuid::new_v4();
        let created = HashMap::from([(
            "new-user".to_string(),
            ScimBulkResourceRef {
                resource_type: ScimBulkResourceType::User,
                id: user_id,
            },
        )]);
        let operation: ScimBulkOperation = serde_json::from_value(serde_json::json!({
            "method": "POST",
            "path": "/Groups",
            "data": {
                "displayName": "Bulk Group",
                "members": [{"value": "bulkId:new-user", "type": "User"}]
            }
        }))
        .unwrap();

        let resolved = resolve_bulk_operation(&operation, &created).unwrap();
        assert_eq!(
            resolved.data.unwrap()["members"][0]["value"],
            user_id.to_string()
        );

        let path_reference: ScimBulkOperation = serde_json::from_value(serde_json::json!({
            "method": "DELETE",
            "path": "/Users/bulkId:new-user"
        }))
        .unwrap();
        assert_eq!(
            resolve_bulk_operation(&path_reference, &created)
                .unwrap()
                .path,
            format!("/Users/{user_id}")
        );

        let unknown: ScimBulkOperation = serde_json::from_value(serde_json::json!({
            "method": "DELETE",
            "path": "/Users/bulkId:later"
        }))
        .unwrap();
        assert!(resolve_bulk_operation(&unknown, &created).is_err());
    }

    #[test]
    fn scim_bulk_request_uses_rfc_capitalized_operations_field() {
        let request: ScimBulkRequest = serde_json::from_value(serde_json::json!({
            "schemas": [BULK_REQUEST_SCHEMA],
            "failOnErrors": 1,
            "Operations": [{"method": "DELETE", "path": "/Users/00000000-0000-0000-0000-000000000001"}]
        }))
        .unwrap();
        assert_eq!(request.operations.len(), 1);
        assert_eq!(request.fail_on_errors, Some(1));
    }

    #[tokio::test]
    async fn scim_error_uses_rfc7644_error_schema_and_media_type() {
        let response =
            ScimApiError(AppError::Validation("invalid SCIM filter".into())).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/scim+json"
        );
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["schemas"][0], ERROR_SCHEMA);
        assert_eq!(body["status"], "400");
        assert_eq!(body["scimType"], "invalidFilter");
        assert_eq!(body["detail"], "invalid SCIM filter");
    }

    #[tokio::test]
    async fn scim_internal_errors_do_not_expose_database_details() {
        let response =
            ScimApiError(AppError::Other("private backend detail".into())).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["detail"], "Internal server error");
        assert_eq!(body["status"], "500");
        assert!(body.get("scimType").is_none());
    }

    async fn assert_scim_error(response: Response, expected_status: StatusCode) {
        assert_eq!(response.status(), expected_status);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/scim+json"
        );
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["schemas"][0], ERROR_SCHEMA);
        assert_eq!(body["status"], expected_status.as_u16().to_string());
        assert!(body["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()));
    }

    #[tokio::test]
    async fn scim_route_formats_json_query_and_path_extractor_errors() {
        use axum::routing::get;
        use tower::ServiceExt;

        let app = axum::Router::new()
            .route(
                "/api/v1/scim/v2/organizations/demo/Users",
                get(|Query(_count): Query<u32>| async { StatusCode::NO_CONTENT })
                    .post(|Json(_body): Json<Value>| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/scim/v2/organizations/demo/Users/{user_id}",
                get(|Path(_user_id): Path<Uuid>| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/scim/v2/organizations/demo/Users/success",
                get(|| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/scim/v2/organizations/demo/Users/preserved",
                get(|| async {
                    ScimApiError(AppError::Validation("keep this detail".into())).into_response()
                }),
            )
            .route("/health", get(|| async { StatusCode::NOT_FOUND }))
            .layer(axum::middleware::from_fn(normalize_scim_extractor_errors));

        let malformed_json = Request::builder()
            .method("POST")
            .uri("/api/v1/scim/v2/organizations/demo/Users")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from("{"))
            .unwrap();
        assert_scim_error(
            app.clone().oneshot(malformed_json).await.unwrap(),
            StatusCode::BAD_REQUEST,
        )
        .await;

        let malformed_query = Request::builder()
            .uri("/api/v1/scim/v2/organizations/demo/Users?count=not-a-number")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_scim_error(
            app.clone().oneshot(malformed_query).await.unwrap(),
            StatusCode::BAD_REQUEST,
        )
        .await;

        let malformed_path = Request::builder()
            .uri("/api/v1/scim/v2/organizations/demo/Users/not-a-uuid")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_scim_error(
            app.clone().oneshot(malformed_path).await.unwrap(),
            StatusCode::BAD_REQUEST,
        )
        .await;

        let unsupported_media = Request::builder()
            .method("POST")
            .uri("/api/v1/scim/v2/organizations/demo/Users")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        assert_scim_error(
            app.clone().oneshot(unsupported_media).await.unwrap(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        )
        .await;

        let unknown_scim_path = Request::builder()
            .uri("/api/v1/scim/v2/organizations/demo/Unknown")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_scim_error(
            app.clone().oneshot(unknown_scim_path).await.unwrap(),
            StatusCode::NOT_FOUND,
        )
        .await;

        let success = Request::builder()
            .uri("/api/v1/scim/v2/organizations/demo/Users/success")
            .body(axum::body::Body::empty())
            .unwrap();
        let success_response = app.clone().oneshot(success).await.unwrap();
        assert_eq!(success_response.status(), StatusCode::NO_CONTENT);
        assert_ne!(
            success_response.headers().get(header::CONTENT_TYPE),
            Some(&axum::http::HeaderValue::from_static(
                "application/scim+json"
            ))
        );

        let preserved_scim_error = Request::builder()
            .uri("/api/v1/scim/v2/organizations/demo/Users/preserved")
            .body(axum::body::Body::empty())
            .unwrap();
        let preserved_response = app.clone().oneshot(preserved_scim_error).await.unwrap();
        assert_eq!(preserved_response.status(), StatusCode::BAD_REQUEST);
        let preserved_body: Value = serde_json::from_slice(
            &to_bytes(preserved_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(preserved_body["detail"], "keep this detail");

        let non_scim_error = Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let non_scim_response = app.oneshot(non_scim_error).await.unwrap();
        assert_eq!(non_scim_response.status(), StatusCode::NOT_FOUND);
        assert_ne!(
            non_scim_response.headers().get(header::CONTENT_TYPE),
            Some(&axum::http::HeaderValue::from_static(
                "application/scim+json"
            ))
        );
    }

    #[test]
    fn scim_user_sorting_uses_allowlisted_attributes_and_direction() {
        assert_eq!(
            user_sort_clause(Some("displayName"), Some("descending")).unwrap(),
            "lower(si.display_name) DESC, p.id ASC"
        );
        assert_eq!(
            user_sort_clause(Some("meta.created"), None).unwrap(),
            "p.created_at ASC, p.id ASC"
        );
        assert!(user_sort_clause(Some("displayName; DROP TABLE users"), None).is_err());
        assert!(user_sort_clause(Some("userName"), Some("sideways")).is_err());
    }

    #[tokio::test]
    async fn unique_constraint_conflicts_are_scim_uniqueness_errors() {
        for constraint in [
            PRINCIPAL_HANDLE_UNIQUE_CONSTRAINT,
            SCIM_EXTERNAL_ID_UNIQUE_CONSTRAINT,
        ] {
            let error = scim_user_creation_conflict(Some(constraint)).unwrap();
            let response = ScimApiError(error).into_response();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/scim+json"
            );
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["scimType"], "uniqueness");
        }
        assert!(scim_user_creation_conflict(Some("other_constraint")).is_none());
    }
}
