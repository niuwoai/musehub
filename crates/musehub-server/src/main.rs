//! MuseHub server: minimal HTTP surface for liveness, readiness and version.

use std::process::ExitCode;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use musehub_core::{version, Config, ErrorCode};

const PROCESS_OK: &str = "ok";
const NOT_CONFIGURED: &str = "not_configured";
const DATABASE_ERROR: &str = "error";

#[derive(Clone)]
struct AppState {
    database: Option<PgPool>,
    database_configured: bool,
}

#[derive(Serialize)]
struct HealthBody {
    status: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct ReadinessChecks {
    process: &'static str,
    database: &'static str,
    git_storage: &'static str,
}

#[derive(Serialize)]
struct ReadinessBody {
    ready: bool,
    version: &'static str,
    checks: ReadinessChecks,
}

#[derive(Serialize)]
struct VersionBody {
    version: &'static str,
}

async fn healthz() -> Json<HealthBody> {
    Json(HealthBody {
        status: PROCESS_OK,
        version: version(),
    })
}

async fn readyz(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let database = match &state.database {
        None if state.database_configured => DATABASE_ERROR,
        None => NOT_CONFIGURED,
        Some(pool) => match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query("SELECT 1").execute(pool),
        )
        .await
        {
            Ok(Ok(_)) => PROCESS_OK,
            Ok(Err(_)) | Err(_) => DATABASE_ERROR,
        },
    };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ReadinessBody {
            ready: false,
            version: version(),
            checks: ReadinessChecks {
                process: PROCESS_OK,
                database,
                git_storage: NOT_CONFIGURED,
            },
        }),
    )
}

async fn version_endpoint() -> Json<VersionBody> {
    Json(VersionBody { version: version() })
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version_endpoint))
        .with_state(state)
        .layer(middleware::from_fn(with_app_version))
}

async fn with_app_version(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        HeaderName::from_static("x-app-version"),
        HeaderValue::from_static(version()),
    );
    response
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(code = error.code(), error = %error, "invalid configuration");
            return ExitCode::from(78);
        }
    };

    let database = match config.database_url() {
        Some(database_url) => match PgPoolOptions::new()
            .max_connections(5)
            .connect_lazy(database_url)
        {
            Ok(pool) => Some(pool),
            Err(_) => {
                tracing::error!("invalid PostgreSQL connection configuration");
                None
            }
        },
        None => None,
    };
    let state = Arc::new(AppState {
        database,
        database_configured: config.database_url().is_some(),
    });

    let listener = match TcpListener::bind(config.bind()).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(bind = %config.bind(), error = %error, "cannot bind listen address");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(
        version = version(),
        bind = %config.bind(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header::CONTENT_TYPE, HeaderMap};
    use http_body_util::BodyExt;
    use musehub_core::VERSION_HEADER;
    use serde_json::Value;
    use tower::ServiceExt;

    async fn call(path: &str) -> (StatusCode, HeaderMap, Value) {
        call_with_state(
            path,
            AppState {
                database: None,
                database_configured: false,
            },
        )
        .await
    }

    async fn call_with_state(path: &str, state: AppState) -> (StatusCode, HeaderMap, Value) {
        let request = Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("build request");
        let response = app(Arc::new(state))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("read body")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, headers, body)
    }

    fn header_version(headers: &HeaderMap) -> Option<&str> {
        headers
            .get(VERSION_HEADER)
            .and_then(|value| value.to_str().ok())
    }

    #[tokio::test]
    async fn healthz_reports_process_alive() {
        let (status, headers, body) = call("/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], PROCESS_OK);
        assert_eq!(body["version"], version());
        assert_eq!(header_version(&headers), Some(version()));
    }

    #[tokio::test]
    async fn readyz_is_not_ready_without_dependencies() {
        let (status, headers, body) = call("/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["ready"], false);
        assert_eq!(body["version"], version());
        assert_eq!(body["checks"]["process"], PROCESS_OK);
        assert_eq!(body["checks"]["database"], NOT_CONFIGURED);
        assert_eq!(body["checks"]["git_storage"], NOT_CONFIGURED);
        assert_eq!(header_version(&headers), Some(version()));
    }

    #[tokio::test]
    async fn readyz_reports_database_error_when_configured_pool_is_unavailable() {
        let (status, _, body) = call_with_state(
            "/readyz",
            AppState {
                database: None,
                database_configured: true,
            },
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["checks"]["database"], DATABASE_ERROR);
        assert_eq!(body["checks"]["git_storage"], NOT_CONFIGURED);
    }

    #[tokio::test]
    async fn version_returns_single_source_version() {
        let (status, headers, body) = call("/version").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "version": version() }));
        assert_eq!(header_version(&headers), Some(version()));
        assert_eq!(
            headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
    }

    #[tokio::test]
    async fn unmatched_route_still_carries_version_header() {
        let (status, headers, _) = call("/does-not-exist").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(header_version(&headers), Some(version()));
    }

    #[test]
    fn version_header_name_is_stable() {
        assert_eq!(VERSION_HEADER, "X-App-Version");
        assert!(!version().is_empty());
    }

    #[test]
    fn server_manifest_version_matches_version_source() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
