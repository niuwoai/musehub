//! Product install profile: demo allows mock compose; production fail-closed without remote workers.

use crate::compose::{compose_mode_from_env, ComposeAdapterMode};
use crate::runner_api::{agent_executor_remote, deploy_executor_remote, pipeline_executor_remote};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductProfile {
    Demo,
    Production,
}

pub fn product_profile_from_env() -> ProductProfile {
    match std::env::var("MUSEHUB_PRODUCT_PROFILE")
        .unwrap_or_else(|_| "demo".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "production" | "prod" => ProductProfile::Production,
        _ => ProductProfile::Demo,
    }
}

/// Called at API startup. Production profile requires remote executors and compose-runner.
pub fn validate_startup_profile() -> Result<(), String> {
    if product_profile_from_env() != ProductProfile::Production {
        return Ok(());
    }
    if !pipeline_executor_remote() {
        return Err(
            "MUSEHUB_PRODUCT_PROFILE=production requires MUSEHUB_PIPELINE_EXECUTOR=remote".into(),
        );
    }
    if !agent_executor_remote() {
        return Err(
            "MUSEHUB_PRODUCT_PROFILE=production requires MUSEHUB_AGENT_EXECUTOR=remote".into(),
        );
    }
    if !deploy_executor_remote() {
        return Err(
            "MUSEHUB_PRODUCT_PROFILE=production requires MUSEHUB_DEPLOY_EXECUTOR=remote".into(),
        );
    }
    match compose_mode_from_env() {
        ComposeAdapterMode::Mock => Err(
            "MUSEHUB_PRODUCT_PROFILE=production forbids MUSEHUB_COMPOSE_ADAPTER_MODE=mock".into(),
        ),
        ComposeAdapterMode::Runner | ComposeAdapterMode::Auto => Ok(()),
    }
}

pub fn profile_label() -> &'static str {
    match product_profile_from_env() {
        ProductProfile::Demo => "demo",
        ProductProfile::Production => "production",
    }
}
