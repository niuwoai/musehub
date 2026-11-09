use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use musehub_app::{boot_reconcile, lease, prepare, product_router, AppState as ProductState};
use musehub_core::version;
use serde::Serialize;
use sqlx::PgPool;
use tower_http::cors::{Any, CorsLayer};

use musehub_core::version as app_version;

#[derive(Clone)]
pub struct AppState {
    pub database: Option<PgPool>,
    pub database_configured: bool,
    pub git_root: Option<PathBuf>,
    pub product: Option<Arc<ProductState>>,
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

pub const PROCESS_OK: &str = "ok";
const NOT_CONFIGURED: &str = "not_configured";
const DATABASE_ERROR: &str = "error";
pub const GIT_NOT_CONFIGURED: &str = NOT_CONFIGURED;

async fn healthz() -> Json<HealthBody> {
    Json(HealthBody {
        status: PROCESS_OK,
        version: app_version(),
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
    let git_storage = match &state.git_root {
        Some(path) if path.is_dir() => PROCESS_OK,
        Some(_) => DATABASE_ERROR,
        None => NOT_CONFIGURED,
    };
    let ready = database == PROCESS_OK && git_storage == PROCESS_OK;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(ReadinessBody {
            ready,
            version: app_version(),
            checks: ReadinessChecks {
                process: PROCESS_OK,
                database,
                git_storage,
            },
        }),
    )
}

async fn version_endpoint() -> Json<VersionBody> {
    Json(VersionBody {
        version: app_version(),
    })
}

pub fn app(state: Arc<AppState>) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version_endpoint))
        .with_state(state.clone())
        .layer(middleware::from_fn(with_app_version));
    if let Some(product) = &state.product {
        router = router.merge(
            product_router(product.clone()).layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any),
            ),
        );
    }
    router
}

async fn with_app_version(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        HeaderName::from_static("x-app-version"),
        HeaderValue::from_static(version()),
    );
    response
}

#[allow(dead_code)]
pub async fn build_product_state(
    pool: PgPool,
    git_root: PathBuf,
    bind_display: String,
    bootstrap_password: Option<&str>,
) -> Result<Arc<ProductState>, Box<dyn std::error::Error + Send + Sync>> {
    prepare(pool.clone(), git_root.clone(), bootstrap_password).await?;
    boot_reconcile(&pool).await?;
    let instance_id = lease::parse_instance_id();
    let reference_adapter_bin = std::env::var("MUSEHUB_REFERENCE_ADAPTER_BIN")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-agent-reference"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_default()
                .join("target/debug/musehub-agent-reference")
        });
    let pipeline_adapter_bin = std::env::var("MUSEHUB_PIPELINE_ADAPTER_BIN")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-pipeline-reference"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_default()
                .join("target/debug/musehub-pipeline-reference")
        });
    let compose_adapter_bin = std::env::var("MUSEHUB_COMPOSE_ADAPTER_BIN")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-compose-mock"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_default()
                .join("target/debug/musehub-compose-mock")
        });
    let compose_runner_bin = std::env::var("MUSEHUB_COMPOSE_RUNNER_BIN")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_musehub-compose-runner"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_default()
                .join("target/debug/musehub-compose-runner")
        });
    let artifact_root = std::env::var("MUSEHUB_ARTIFACT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| git_root.join("../musehub-artifacts"));
    Ok(Arc::new(ProductState {
        pool,
        git_root,
        artifact_root,
        bind_display,
        reference_adapter_bin,
        pipeline_adapter_bin,
        compose_adapter_bin,
        compose_runner_bin,
        instance_id,
        supervisor: Arc::new(musehub_app::agent_supervisor::AgentSupervisor::default()),
        pipeline_supervisor: musehub_app::pipeline_supervisor::PipelineSupervisor::new(),
    }))
}
