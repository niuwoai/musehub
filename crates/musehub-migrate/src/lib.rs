//! Explicit PostgreSQL migrations. The web server does not call this.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use musehub_core::ErrorCode;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

const MIGRATIONS_TABLE: &str = "musehub_schema_migrations";

/// One reversible migration loaded from a numbered SQL pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    version: String,
    name: String,
    up_sql: String,
    down_sql: String,
}

impl Migration {
    /// Stable version used as the applied-migration key.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Name after the numeric prefix.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Result of applying or reverting migrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    applied: Vec<String>,
    skipped: Vec<String>,
}

impl MigrationReport {
    /// Versions changed by this command.
    pub fn applied(&self) -> &[String] {
        &self.applied
    }

    /// Versions already in the requested state.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }
}

/// Timestamp and foreign-key index findings for ordinary tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaCheck {
    missing_timestamps: Vec<String>,
    missing_fk_indexes: Vec<String>,
}

impl SchemaCheck {
    /// Tables missing `created_at` or `updated_at`.
    pub fn missing_timestamps(&self) -> &[String] {
        &self.missing_timestamps
    }

    /// Foreign keys without a leading index.
    pub fn missing_fk_indexes(&self) -> &[String] {
        &self.missing_fk_indexes
    }

    /// Whether every checked table satisfies the BOOT-02 rules.
    pub fn is_ok(&self) -> bool {
        self.missing_timestamps.is_empty() && self.missing_fk_indexes.is_empty()
    }
}

/// Loads `NNNN_name.up.sql` / `NNNN_name.down.sql` pairs.
///
/// # Errors
///
/// Returns an error when a file is missing, unpaired, unreadable, or misnamed.
pub fn load_migrations(directory: &Path) -> Result<Vec<Migration>, MigrateError> {
    let mut ups = Vec::new();
    let mut downs = Vec::new();
    let entries = fs::read_dir(directory).map_err(|source| MigrateError::Io {
        step: "read migrations directory",
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| MigrateError::Io {
            step: "read migrations entry",
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(stem) = file_name.strip_suffix(".up.sql") {
            ups.push((stem.to_string(), path));
        } else if let Some(stem) = file_name.strip_suffix(".down.sql") {
            downs.push((stem.to_string(), path));
        }
    }
    ups.sort();
    downs.sort();
    if ups.len() != downs.len() || ups.iter().zip(&downs).any(|(up, down)| up.0 != down.0) {
        return Err(MigrateError::Unpaired {
            detail: format!("up={:?} down={:?}", names_of(&ups), names_of(&downs)),
        });
    }

    let mut migrations = Vec::with_capacity(ups.len());
    for ((stem, up_path), (_, down_path)) in ups.into_iter().zip(downs) {
        let (version, name) = split_stem(&stem)?;
        migrations.push(Migration {
            version,
            name,
            up_sql: read_sql(&up_path)?,
            down_sql: read_sql(&down_path)?,
        });
    }
    Ok(migrations)
}

/// Opens a PostgreSQL pool. This does not migrate.
///
/// # Errors
///
/// Returns an error when the database rejects the connection.
pub async fn connect(database_url: &str) -> Result<PgPool, MigrateError> {
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(60))
        .connect(database_url)
        .await
        .map_err(|source| MigrateError::Database {
            step: "connect",
            source,
        })
}

/// Applies every migration that is not already recorded, in version order.
///
/// # Errors
///
/// Returns an error when bookkeeping or a migration statement fails. A failed
/// migration is not recorded.
pub async fn migrate_up(
    pool: &PgPool,
    migrations: &[Migration],
) -> Result<MigrationReport, MigrateError> {
    ensure_bookkeeping(pool).await?;
    let applied_versions = applied_versions(pool).await?;
    let mut report = MigrationReport {
        applied: Vec::new(),
        skipped: Vec::new(),
    };
    for migration in migrations {
        if applied_versions
            .iter()
            .any(|version| version == &migration.version)
        {
            report.skipped.push(migration.version.clone());
            continue;
        }
        let mut tx = pool
            .begin()
            .await
            .map_err(|source| MigrateError::Database {
                step: "begin up",
                source,
            })?;
        sqlx::raw_sql(&migration.up_sql)
            .execute(&mut *tx)
            .await
            .map_err(|source| MigrateError::Database {
                step: "apply up",
                source,
            })?;
        sqlx::query(&format!(
            "INSERT INTO {MIGRATIONS_TABLE} (version, name, applied_at) VALUES ($1, $2, CURRENT_TIMESTAMP)"
        ))
        .bind(&migration.version)
        .bind(&migration.name)
        .execute(&mut *tx)
        .await
        .map_err(|source| MigrateError::Database {
            step: "record up",
            source,
        })?;
        tx.commit().await.map_err(|source| MigrateError::Database {
            step: "commit up",
            source,
        })?;
        report.applied.push(migration.version.clone());
    }
    Ok(report)
}

/// Reverts applied migrations in reverse version order.
///
/// # Errors
///
/// Returns an error when a recorded version has no matching down migration,
/// or when a statement fails. A failed down migration stays recorded.
pub async fn migrate_down(
    pool: &PgPool,
    migrations: &[Migration],
) -> Result<MigrationReport, MigrateError> {
    ensure_bookkeeping(pool).await?;
    sqlx::query(
        "SELECT pg_terminate_backend(pid)
         FROM pg_stat_activity
         WHERE datname = current_database()
           AND pid <> pg_backend_pid()",
    )
    .execute(pool)
    .await
    .map_err(|source| MigrateError::Database {
        step: "terminate other backends",
        source,
    })?;
    let applied_versions = applied_versions(pool).await?;
    let mut report = MigrationReport {
        applied: Vec::new(),
        skipped: Vec::new(),
    };
    for version in applied_versions.into_iter().rev() {
        let Some(migration) = migrations.iter().find(|item| item.version == version) else {
            return Err(MigrateError::MissingDown { version });
        };
        let mut tx = pool
            .begin()
            .await
            .map_err(|source| MigrateError::Database {
                step: "begin down",
                source,
            })?;
        sqlx::raw_sql(&migration.down_sql)
            .execute(&mut *tx)
            .await
            .map_err(|source| MigrateError::Database {
                step: "apply down",
                source,
            })?;
        sqlx::query(&format!(
            "DELETE FROM {MIGRATIONS_TABLE} WHERE version = $1"
        ))
        .bind(&migration.version)
        .execute(&mut *tx)
        .await
        .map_err(|source| MigrateError::Database {
            step: "record down",
            source,
        })?;
        tx.commit().await.map_err(|source| MigrateError::Database {
            step: "commit down",
            source,
        })?;
        report.applied.push(migration.version.clone());
    }
    Ok(report)
}

/// Checks ordinary public tables for timestamps and leading foreign-key indexes.
///
/// # Errors
///
/// Returns an error when catalog queries fail.
pub async fn check_schema(pool: &PgPool) -> Result<SchemaCheck, MigrateError> {
    let timestamp_rows = sqlx::query(
        "SELECT c.relname
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public'
           AND c.relkind = 'r'
           AND c.relname <> $1
           AND (
             NOT EXISTS (
               SELECT 1 FROM information_schema.columns created
               WHERE created.table_schema = 'public'
                 AND created.table_name = c.relname
                 AND created.column_name = 'created_at'
                 AND created.data_type = 'timestamp with time zone'
                 AND created.is_nullable = 'NO'
             )
             OR NOT EXISTS (
               SELECT 1 FROM information_schema.columns updated
               WHERE updated.table_schema = 'public'
                 AND updated.table_name = c.relname
                 AND updated.column_name = 'updated_at'
                 AND updated.data_type = 'timestamp with time zone'
                 AND updated.is_nullable = 'NO'
             )
           )
         ORDER BY c.relname",
    )
    .bind(MIGRATIONS_TABLE)
    .fetch_all(pool)
    .await
    .map_err(|source| MigrateError::Database {
        step: "check timestamps",
        source,
    })?;

    let index_rows = sqlx::query(
        "SELECT format('%s(%s)', source.relname, string_agg(attribute.attname, ',' ORDER BY key.ordinality))
         FROM pg_constraint constraint_
         JOIN pg_class source ON source.oid = constraint_.conrelid
         JOIN pg_namespace namespace ON namespace.oid = source.relnamespace
         JOIN LATERAL unnest(constraint_.conkey) WITH ORDINALITY AS key(attnum, ordinality) ON true
         JOIN pg_attribute attribute
           ON attribute.attrelid = source.oid
          AND attribute.attnum = key.attnum
         WHERE constraint_.contype = 'f'
           AND namespace.nspname = 'public'
           AND NOT EXISTS (
             SELECT 1
             FROM pg_index index_
             WHERE index_.indrelid = constraint_.conrelid
               AND index_.indisvalid
               AND (
                 SELECT array_agg(index_.indkey[position]::smallint ORDER BY position)
                 FROM generate_subscripts(index_.indkey, 1) AS position
                 WHERE position <= array_length(constraint_.conkey, 1)
               ) = constraint_.conkey
           )
         GROUP BY source.relname, constraint_.oid
         ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .map_err(|source| MigrateError::Database {
        step: "check foreign-key indexes",
        source,
    })?;

    Ok(SchemaCheck {
        missing_timestamps: column(&timestamp_rows),
        missing_fk_indexes: column(&index_rows),
    })
}

/// Returns public base table names, excluding migration bookkeeping.
///
/// # Errors
///
/// Returns an error when the catalog query fails.
pub async fn business_tables(pool: &PgPool) -> Result<Vec<String>, MigrateError> {
    let rows = sqlx::query(
        "SELECT relname
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relkind = 'r' AND c.relname <> $1
         ORDER BY relname",
    )
    .bind(MIGRATIONS_TABLE)
    .fetch_all(pool)
    .await
    .map_err(|source| MigrateError::Database {
        step: "list tables",
        source,
    })?;
    Ok(column(&rows))
}

fn names_of(entries: &[(String, PathBuf)]) -> Vec<&str> {
    entries.iter().map(|(name, _)| name.as_str()).collect()
}

fn split_stem(stem: &str) -> Result<(String, String), MigrateError> {
    let Some((version, name)) = stem.split_once('_') else {
        return Err(MigrateError::InvalidName {
            name: stem.to_string(),
        });
    };
    if version.len() != 4 || !version.chars().all(|char| char.is_ascii_digit()) || name.is_empty() {
        return Err(MigrateError::InvalidName {
            name: stem.to_string(),
        });
    }
    Ok((version.to_string(), name.to_string()))
}

fn read_sql(path: &Path) -> Result<String, MigrateError> {
    fs::read_to_string(path).map_err(|source| MigrateError::Io {
        step: "read migration sql",
        source,
    })
}

fn column(rows: &[sqlx::postgres::PgRow]) -> Vec<String> {
    rows.iter().map(|row| row.get::<String, _>(0)).collect()
}

async fn ensure_bookkeeping(pool: &PgPool) -> Result<(), MigrateError> {
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {MIGRATIONS_TABLE} (
            version TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TIMESTAMPTZ NOT NULL
        )"
    ))
    .execute(pool)
    .await
    .map_err(|source| MigrateError::Database {
        step: "create migration table",
        source,
    })?;
    Ok(())
}

async fn applied_versions(pool: &PgPool) -> Result<Vec<String>, MigrateError> {
    let rows = sqlx::query(&format!(
        "SELECT version FROM {MIGRATIONS_TABLE} ORDER BY version"
    ))
    .fetch_all(pool)
    .await
    .map_err(|source| MigrateError::Database {
        step: "list applied migrations",
        source,
    })?;
    Ok(column(&rows))
}

/// Migration failures. Every variant maps to `migrate_failed`.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("migration files are not paired: {detail}")]
    Unpaired { detail: String },
    #[error("invalid migration name `{name}`; expected NNNN_name")]
    InvalidName { name: String },
    #[error("applied migration {version} has no down file")]
    MissingDown { version: String },
    #[error("migration step `{step}` failed: {source}")]
    Io {
        step: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("migration step `{step}` failed: {source}")]
    Database {
        step: &'static str,
        #[source]
        source: sqlx::Error,
    },
    #[error("schema check failed: {detail}")]
    Schema { detail: String },
    #[error("migration argument invalid: {detail}")]
    Usage { detail: String },
}

impl ErrorCode for MigrateError {
    fn code(&self) -> &'static str {
        "migrate_failed"
    }
}

/// Unix time in whole seconds, used only to name an isolated database.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_is_stable() {
        let error = MigrateError::Usage {
            detail: "missing url".to_string(),
        };
        assert_eq!(error.code(), "migrate_failed");
    }

    #[test]
    fn migration_name_requires_four_digits() {
        assert!(split_stem("0001_common").is_ok());
        assert!(split_stem("1_common").is_err());
        assert!(split_stem("0001").is_err());
    }
}
