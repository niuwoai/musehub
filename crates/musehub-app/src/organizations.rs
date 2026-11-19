use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct OrganizationView {
    id: Uuid,
    slug: String,
    display_name: String,
    role: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateOrganizationRequest {
    slug: String,
    display_name: String,
}

pub async fn list_organizations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<OrganizationView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String)>(
        "SELECT o.id, o.slug, o.display_name, m.role
         FROM organizations o
         JOIN memberships m ON m.org_id = o.id
         WHERE m.principal_id = $1 AND m.status = 'active' AND o.status = 'active'
         ORDER BY o.created_at, o.slug_normalized",
    )
    .bind(principal_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, slug, display_name, role)| OrganizationView {
                id,
                slug,
                display_name,
                role,
            })
            .collect(),
    ))
}

pub async fn create_organization(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateOrganizationRequest>,
) -> Result<(StatusCode, Json<OrganizationView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let slug = body.slug.trim().to_ascii_lowercase();
    let display_name = body.display_name.trim();
    validate_org_slug(&slug)?;
    if display_name.is_empty() || display_name.chars().count() > 120 {
        return Err(AppError::Validation(
            "display_name must contain 1 to 120 characters".to_string(),
        ));
    }

    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let insert_org = sqlx::query(
        "INSERT INTO organizations (id, slug, slug_normalized, display_name, status)
         VALUES ($1, $2, $2, $3, 'active')",
    )
    .bind(id)
    .bind(&slug)
    .bind(display_name)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert_org {
        if error
            .as_database_error()
            .and_then(|database_error| database_error.code())
            .as_deref()
            == Some("23505")
        {
            return Err(AppError::Conflict(format!(
                "organization slug already exists: {slug}"
            )));
        }
        return Err(error.into());
    }
    sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status)
         VALUES ($1, $2, $3, 'owner', 'active')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    let request_id = crate::audit::request_id_from_headers(&headers);
    crate::audit::record_in_tx(
        &mut tx,
        Some(id),
        None,
        principal_id,
        None,
        "org.create",
        "organization",
        id,
        request_id,
        "allow",
        serde_json::json!({ "slug": slug, "display_name": display_name }),
    )
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(OrganizationView {
            id,
            slug,
            display_name: display_name.to_string(),
            role: "owner".to_string(),
        }),
    ))
}

fn validate_org_slug(slug: &str) -> Result<(), AppError> {
    let valid = (3..=48).contains(&slug.len())
        && slug
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && slug
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(AppError::Validation(
            "slug must be 3 to 48 lowercase letters, digits, or internal hyphens".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::validate_org_slug;

    #[test]
    fn organization_slug_validation_accepts_and_rejects_expected_values() {
        for slug in ["team-1", "abc", "a123"] {
            assert!(validate_org_slug(slug).is_ok(), "{slug}");
        }
        for slug in ["ab", "-team", "team-", "has space", "UPPER", "a_b"] {
            assert!(validate_org_slug(slug).is_err(), "{slug}");
        }
    }
}
