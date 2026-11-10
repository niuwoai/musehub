//! Production profile fail-closed unless remote executors and compose-runner configured.

#[test]
fn production_profile_rejects_mock_compose() {
    std::env::set_var("MUSEHUB_PRODUCT_PROFILE", "production");
    std::env::set_var("MUSEHUB_PIPELINE_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_AGENT_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_DEPLOY_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_COMPOSE_ADAPTER_MODE", "mock");
    assert!(musehub_app::product_profile::validate_startup_profile().is_err());
    std::env::remove_var("MUSEHUB_PRODUCT_PROFILE");
    std::env::remove_var("MUSEHUB_PIPELINE_EXECUTOR");
    std::env::remove_var("MUSEHUB_AGENT_EXECUTOR");
    std::env::remove_var("MUSEHUB_DEPLOY_EXECUTOR");
    std::env::remove_var("MUSEHUB_COMPOSE_ADAPTER_MODE");
}

#[test]
fn production_profile_accepts_runner_compose() {
    std::env::set_var("MUSEHUB_PRODUCT_PROFILE", "production");
    std::env::set_var("MUSEHUB_PIPELINE_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_AGENT_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_DEPLOY_EXECUTOR", "remote");
    std::env::set_var("MUSEHUB_COMPOSE_ADAPTER_MODE", "runner");
    assert!(musehub_app::product_profile::validate_startup_profile().is_ok());
    std::env::remove_var("MUSEHUB_PRODUCT_PROFILE");
    std::env::remove_var("MUSEHUB_PIPELINE_EXECUTOR");
    std::env::remove_var("MUSEHUB_AGENT_EXECUTOR");
    std::env::remove_var("MUSEHUB_DEPLOY_EXECUTOR");
    std::env::remove_var("MUSEHUB_COMPOSE_ADAPTER_MODE");
}
