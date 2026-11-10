//! Command that applies or reverts paired PostgreSQL migrations.
//!
//! The server never invokes this on startup.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use musehub_core::ErrorCode;
use musehub_migrate::{
    check_schema, connect, load_migrations, migrate_down, migrate_up, MigrateError,
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), MigrateError> {
    let mut args = env::args().skip(1);
    let command = args.next().ok_or_else(|| MigrateError::Usage {
        detail: "usage: musehub-migrate <up|down> <migrations-dir> <database-url>".to_string(),
    })?;
    let directory = args.next().ok_or_else(|| MigrateError::Usage {
        detail: "migrations directory is required".to_string(),
    })?;
    let database_url = args.next().ok_or_else(|| MigrateError::Usage {
        detail: "database url is required".to_string(),
    })?;
    if args.next().is_some() {
        return Err(MigrateError::Usage {
            detail: "too many arguments".to_string(),
        });
    }

    let migrations = load_migrations(&PathBuf::from(directory))?;
    let pool = connect(&database_url).await?;
    let report = match command.as_str() {
        "up" => migrate_up(&pool, &migrations).await?,
        "down" => migrate_down(&pool, &migrations).await?,
        _ => {
            return Err(MigrateError::Usage {
                detail: "command must be up or down".to_string(),
            })
        }
    };
    println!(
        "command={command} applied={} skipped={}",
        report.applied().join(","),
        report.skipped().join(",")
    );
    if command == "up" {
        let check = check_schema(&pool).await?;
        if !check.is_ok() {
            return Err(MigrateError::Schema {
                detail: format!(
                    "timestamps={:?} foreign_keys={:?}",
                    check.missing_timestamps(),
                    check.missing_fk_indexes()
                ),
            });
        }
        println!("schema_check=ok");
    }
    Ok(())
}
