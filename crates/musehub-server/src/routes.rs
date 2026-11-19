use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use musehub_app::{
    boot_reconcile, compose_mode_from_env, lease, prepare, product_profile, product_router,
    runner_api, AppState as ProductState, ComposeAdapterMode,
};
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
    database_role: &'static str,
    git_storage: &'static str,
    product_api: &'static str,
    /// `mock` default in product image; use `runner` + verified OCI path for real compose.
    compose_adapter_mode: &'static str,
    pipeline_executor: &'static str,
    agent_executor: &'static str,
    product_profile: &'static str,
}

fn compose_adapter_mode_label() -> &'static str {
    match compose_mode_from_env() {
        ComposeAdapterMode::Mock => "mock",
        ComposeAdapterMode::Runner => "runner",
        ComposeAdapterMode::Auto => "auto",
    }
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
    let (database, database_role) = match &state.database {
        None if state.database_configured => (DATABASE_ERROR, DATABASE_ERROR),
        None => (NOT_CONFIGURED, NOT_CONFIGURED),
        Some(pool) => match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query_scalar::<_, String>(
                "SELECT CASE
                    WHEN pg_is_in_recovery() THEN 'standby'
                    WHEN current_setting('transaction_read_only')::boolean THEN 'read_only'
                    ELSE 'primary'
                 END",
            )
            .fetch_one(pool),
        )
        .await
        {
            Ok(Ok(role)) if role == "primary" => (PROCESS_OK, "primary"),
            Ok(Ok(role)) if role == "standby" => ("standby", "standby"),
            Ok(Ok(_)) => ("read_only", "read_only"),
            Ok(Err(_)) | Err(_) => (DATABASE_ERROR, DATABASE_ERROR),
        },
    };
    let git_storage = match &state.git_root {
        Some(path) if path.is_dir() => PROCESS_OK,
        Some(_) => DATABASE_ERROR,
        None => NOT_CONFIGURED,
    };
    let product_api = if state.product.is_some() {
        PROCESS_OK
    } else {
        DATABASE_ERROR
    };
    let ready = database == PROCESS_OK && git_storage == PROCESS_OK && product_api == PROCESS_OK;
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
                database_role,
                git_storage,
                product_api,
                compose_adapter_mode: compose_adapter_mode_label(),
                pipeline_executor: if runner_api::pipeline_executor_remote() {
                    "remote"
                } else {
                    "in_process"
                },
                agent_executor: if runner_api::agent_executor_remote() {
                    "remote"
                } else {
                    "in_process"
                },
                product_profile: product_profile::profile_label(),
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
        .with_state(state.clone());
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
        .layer(middleware::from_fn_with_state(
            state,
            require_primary_for_mutations,
        ))
        .layer(middleware::from_fn(with_app_version))
}

async fn require_primary_for_mutations(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method();
    let path = request.uri().path();
    let safe_method = method == Method::GET
        || method == Method::HEAD
        || method == Method::OPTIONS
        || method == Method::TRACE;
    let state_changing_get = matches!(
        path,
        "/api/v1/auth/oidc/start"
            | "/api/v1/auth/oidc/callback"
            | "/api/v1/auth/github/start"
            | "/api/v1/auth/github/callback"
            | "/api/v1/auth/saml/start"
    );
    if safe_method && !state_changing_get {
        return next.run(request).await;
    }

    let Some(pool) = &state.database else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database_not_primary" })),
        )
            .into_response();
    };
    let primary = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        sqlx::query_scalar::<_, bool>(
            "SELECT NOT pg_is_in_recovery()
             AND NOT current_setting('transaction_read_only')::boolean",
        )
        .fetch_one(pool),
    )
    .await;
    match primary {
        Ok(Ok(true)) => next.run(request).await,
        Ok(Ok(false)) | Ok(Err(_)) | Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database_not_primary" })),
        )
            .into_response(),
    }
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
    oidc_config: Option<&musehub_core::OidcConfig>,
    github_oauth_config: Option<&musehub_core::GithubOAuthConfig>,
    cookie_secure: bool,
) -> Result<Arc<ProductState>, Box<dyn std::error::Error + Send + Sync>> {
    let oidc_provider = if let Some(oidc) = oidc_config {
        Some(Arc::new(
            musehub_app::oidc::OidcProvider::discover(
                oidc.issuer_url(),
                oidc.client_id(),
                oidc.client_secret(),
                oidc.redirect_uri(),
                cookie_secure,
            )
            .await?,
        ))
    } else {
        None
    };
    let saml_provider = musehub_app::saml::SamlProvider::from_env()?.map(Arc::new);
    let github_oauth_provider = github_oauth_config
        .map(|config| musehub_app::github_oauth::GithubOAuthProvider::new(config, cookie_secure))
        .transpose()?
        .map(Arc::new);
    let database_not_writable = sqlx::query_scalar::<_, bool>(
        "SELECT pg_is_in_recovery()
         OR current_setting('transaction_read_only')::boolean",
    )
    .fetch_one(&pool)
    .await?;
    if database_not_writable {
        return Err(std::io::Error::other(
            "database endpoint is read-only; refusing control-plane initialization",
        )
        .into());
    }
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
        require_pipeline_oci: musehub_app::require_pipeline_oci_from_env(),
        mcp_pat_slot: Arc::new(tokio::sync::RwLock::new(None)),
        mcp_request_gate: Arc::new(tokio::sync::Mutex::new(())),
        oidc_provider,
        saml_provider,
        github_oauth_provider,
        cookie_secure,
    }))
}
