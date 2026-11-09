//! Local artifact storage (first-version default).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::security::validate_artifact_name;

pub const MAX_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;

pub fn artifact_storage_dir(
    root: &Path,
    org_id: Uuid,
    repo_id: Uuid,
    artifact_id: Uuid,
) -> PathBuf {
    root.join(org_id.to_string())
        .join(repo_id.to_string())
        .join(artifact_id.to_string())
}

pub fn safe_join(base: &Path, file_name: &str) -> Result<PathBuf, AppError> {
    validate_artifact_name(file_name).map_err(AppError::Validation)?;
    let joined = base.join(file_name);
    if joined.parent().is_some_and(|parent| parent != base) {
        return Err(AppError::Validation(
            "path outside artifact dir".to_string(),
        ));
    }
    Ok(joined)
}

pub fn sha256_file(path: &Path) -> Result<(String, u64), AppError> {
    let bytes = std::fs::read(path).map_err(|e| AppError::Other(e.to_string()))?;
    if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err(AppError::Validation("artifact too large".to_string()));
    }
    let digest = Sha256::digest(&bytes);
    let sha256 = digest
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>();
    Ok((sha256, bytes.len() as u64))
}

pub async fn finalize_artifact(
    pool: &PgPool,
    artifact_id: Uuid,
    sha256: &str,
    byte_length: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE artifacts SET state = 'ready', sha256 = $2, byte_length = $3,
         updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND state = 'uploading'",
    )
    .bind(artifact_id)
    .bind(sha256)
    .bind(byte_length)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn quarantine_artifact(pool: &PgPool, artifact_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE artifacts SET state = 'quarantined', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(artifact_id)
    .execute(pool)
    .await?;
    Ok(())
}
