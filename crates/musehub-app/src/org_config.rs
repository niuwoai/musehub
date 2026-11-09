//! Persisted org-level preferences (non-secret).

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

const DEFAULT_COLLAB_ROLE: &str = "reader";

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct OrgPreferences {
    #[serde(default)]
    pub default_collaborator_role: String,
}

impl OrgPreferences {
    pub fn normalize(&mut self) -> Result<(), AppError> {
        if self.default_collaborator_role.is_empty() {
            self.default_collaborator_role = DEFAULT_COLLAB_ROLE.to_string();
        }
        parse_repo_role(Some(self.default_collaborator_role.clone()))?;
        Ok(())
    }
}

pub fn parse_repo_role(role: Option<String>) -> Result<&'static str, AppError> {
    match role.as_deref().unwrap_or(DEFAULT_COLLAB_ROLE) {
        "reader" => Ok("reader"),
        "developer" => Ok("developer"),
        "maintainer" => Ok("maintainer"),
        other => Err(AppError::Validation(format!("invalid repo role: {other}"))),
    }
}

pub async fn load_preferences(pool: &PgPool, org_id: Uuid) -> Result<OrgPreferences, AppError> {
    let row = sqlx::query_as::<_, (serde_json::Value,)>(
        "SELECT preferences FROM organizations WHERE id = $1",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;
    let mut prefs: OrgPreferences = serde_json::from_value(row.0).unwrap_or_default();
    prefs.normalize().ok();
    Ok(prefs)
}

pub async fn save_preferences(
    pool: &PgPool,
    org_id: Uuid,
    prefs: &OrgPreferences,
) -> Result<(), AppError> {
    let mut copy = prefs.clone();
    copy.normalize()?;
    let json = serde_json::to_value(&copy).map_err(|e| AppError::Other(e.to_string()))?;
    sqlx::query(
        "UPDATE organizations SET preferences = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(org_id)
    .bind(json)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_display_name(pool: &PgPool, org_id: Uuid) -> Result<String, AppError> {
    let row = sqlx::query_as::<_, (String,)>("SELECT display_name FROM organizations WHERE id = $1")
        .bind(org_id)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

pub async fn save_display_name(
    pool: &PgPool,
    org_id: Uuid,
    display_name: &str,
) -> Result<(), AppError> {
    let name = display_name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation("invalid display_name".to_string()));
    }
    sqlx::query(
        "UPDATE organizations SET display_name = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(org_id)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}
