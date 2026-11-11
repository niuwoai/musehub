//! Input validation and bootstrap policy.

const MIN_BOOTSTRAP_PASSWORD_LEN: usize = 16;

const WEAK_BOOTSTRAP: &[&str] = &[
    "password",
    "admin",
    "changeme",
    "ci-bootstrap-password",
    "dev-change-me",
];

/// Bootstrap is allowed only on an empty `principals` table with an explicit strong password.
pub fn validate_bootstrap_password(password: &str) -> Result<(), String> {
    let password = password.trim();
    if password.len() < MIN_BOOTSTRAP_PASSWORD_LEN {
        return Err(format!(
            "bootstrap password must be at least {MIN_BOOTSTRAP_PASSWORD_LEN} characters"
        ));
    }
    let lower = password.to_lowercase();
    if WEAK_BOOTSTRAP.iter().any(|weak| lower.contains(weak)) {
        return Err("bootstrap password is too weak".to_string());
    }
    Ok(())
}

/// Git smart-HTTP suffix must not escape the repository.
pub fn validate_git_suffix(suffix: &str) -> Result<(), String> {
    if suffix.contains("..") || suffix.contains('\0') {
        return Err("invalid git path".to_string());
    }
    if suffix.starts_with('/') {
        return Err("invalid git path".to_string());
    }
    Ok(())
}

/// Agent work branches must stay under `agent/` and must not target the default branch.
pub fn validate_agent_work_branch(branch: &str, default_branch: &str) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() || branch == default_branch {
        return Err("agent branch cannot be the default branch".to_string());
    }
    if branch == "main" || branch == "master" {
        return Err("agent branch cannot target a protected default name".to_string());
    }
    let prefix = "agent/";
    if !branch.starts_with(prefix) || branch.len() <= prefix.len() {
        return Err("agent branch must be under agent/".to_string());
    }
    if branch.contains("..") || branch.contains(' ') {
        return Err("invalid agent branch".to_string());
    }
    Ok(())
}

/// PR-agent-fix may work on the PR `source_branch` at a bound `head_oid` (not only `agent/` branches).
pub fn validate_pr_agent_fix_work_branch(branch: &str) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("fix branch must not be empty".to_string());
    }
    if branch.contains("..") || branch.contains(' ') {
        return Err("invalid fix branch".to_string());
    }
    if branch == "main" || branch == "master" {
        return Err("cannot run PR fix directly on default branch name".to_string());
    }
    Ok(())
}

/// Artifact names must be a single path segment without traversal.
pub fn validate_artifact_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err("invalid artifact name".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
        return Err("invalid artifact name".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("invalid artifact name".to_string());
    }
    Ok(())
}

pub fn validate_git_oid(oid: &str) -> Result<(), String> {
    if oid.len() != 40 || !oid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid git oid".to_string());
    }
    Ok(())
}

/// Mock compose health probe only allows explicit mock scheme or loopback http(s) with port.
/// Callers cannot supply compose YAML — only a health check URL on localhost.
/// OCI image reference must be pinned by digest (`name@sha256:64hex`).
pub fn validate_oci_image_ref(image: &str) -> Result<(), String> {
    let image = image.trim();
    if image.len() > 512 {
        return Err("oci image ref too long".to_string());
    }
    let Some((_, digest)) = image.split_once("@sha256:") else {
        return Err("oci image ref must include @sha256:digest".to_string());
    };
    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("oci image digest must be 64 hex chars".to_string());
    }
    Ok(())
}

pub fn validate_compose_health_target(target: &str) -> Result<(), String> {
    let target = target.trim();
    if target == "mock://healthy" {
        return Ok(());
    }
    for prefix in ["http://127.0.0.1:", "http://localhost:"] {
        if let Some(rest) = target.strip_prefix(prefix) {
            let (port, suffix) = match rest.split_once('/') {
                Some((p, s)) => (p, Some(s)),
                None => (rest, None),
            };
            if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if let Some(path) = suffix {
                if path.contains("..") || path.contains('@') {
                    return Err("health target not in allowlist".to_string());
                }
            }
            return Ok(());
        }
    }
    Err("health target not in allowlist".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_weak_bootstrap() {
        assert!(validate_bootstrap_password("short").is_err());
        assert!(validate_bootstrap_password("ci-bootstrap-password-extra!!").is_err());
    }

    #[test]
    fn git_suffix_rejects_traversal() {
        assert!(validate_git_suffix("../etc/passwd").is_err());
        assert!(validate_git_suffix("info/refs").is_ok());
    }

    #[test]
    fn agent_branch_must_not_be_main() {
        assert!(validate_agent_work_branch("main", "main").is_err());
        assert!(validate_agent_work_branch("agent/run-1", "main").is_ok());
    }

    #[test]
    fn compose_health_target_allowlist() {
        assert!(validate_oci_image_ref(
            "docker.io/library/nginx@sha256:0000000000000000000000000000000000000000000000000000000000000001"
        )
        .is_ok());
        assert!(validate_oci_image_ref("nginx:latest").is_err());
        assert!(validate_compose_health_target("mock://healthy").is_ok());
        assert!(validate_compose_health_target("http://127.0.0.1:18080/").is_ok());
        assert!(validate_compose_health_target("http://evil.example/").is_err());
    }
}
