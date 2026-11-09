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
}
