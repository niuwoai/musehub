//! MuseHub server entrypoint.

use std::process::ExitCode;
use std::sync::Arc;

use musehub_core::{Config, ErrorCode};
use musehub_server::{app, build_product_state, AppState};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    if let Err(msg) = musehub_app::product_profile::validate_startup_profile() {
        tracing::error!(error = %msg, "product profile validation failed");
        return ExitCode::from(78);
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(code = error.code(), error = %error, "invalid configuration");
            return ExitCode::from(78);
        }
    };

    let database = match config.database_url() {
        Some(database_url) => match PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
        {
            Ok(pool) => Some(pool),
            Err(error) => {
                tracing::error!(error = %error, "cannot connect database");
                None
            }
        },
        None => None,
    };

    let git_root = config.git_root().cloned();
    let product = match (&database, &git_root, config.session_secret()) {
        (Some(pool), Some(root), Some(_secret)) => {
            let bind_display = config.bind().to_string();
            match build_product_state(
                pool.clone(),
                root.clone(),
                bind_display,
                config.bootstrap_admin_password(),
            )
            .await
            {
                Ok(state) => Some(state),
                Err(error) => {
                    tracing::error!(error = %error, "product routes disabled");
                    None
                }
            }
        }
        _ => None,
    };

    let state = Arc::new(AppState {
        database,
        database_configured: config.database_url().is_some(),
        git_root,
        product,
    });

    let listener = match TcpListener::bind(config.bind()).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(bind = %config.bind(), error = %error, "cannot bind listen address");
            return ExitCode::FAILURE;
        }
    };

    if let Ok(path) = std::env::var(musehub_core::LISTEN_PORT_FILE_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            match listener.local_addr() {
                Ok(addr) => {
                    if let Err(error) = std::fs::write(path, addr.port().to_string()) {
                        tracing::error!(path = %path, error = %error, "cannot write listen port file");
                        return ExitCode::FAILURE;
                    }
                }
                Err(error) => {
                    tracing::error!(error = %error, "cannot read bound listen address");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    tracing::info!(
        version = musehub_core::version(),
        bind = %config.bind(),
        product_api = state.product.is_some(),
        "musehub-server starting"
    );

    if let Err(error) = axum::serve(listener, app(state)).await {
        tracing::error!(error = %error, "musehub-server stopped with error");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
