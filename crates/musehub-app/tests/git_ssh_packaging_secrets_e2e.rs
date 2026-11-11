//! Product git-ssh packaging invariants: no DB literals in world-readable scripts.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::require_git_ssh_transport_prereqs;

#[tokio::test]
async fn git_ssh_public_scripts_contain_no_database_credentials() {
    require_git_ssh_transport_prereqs();
    let _f = git_ssh_fixture("sshpack").await;
    let login = Path::new("/usr/local/lib/musehub/musehub-git-login.sh");
    assert!(
        login.exists(),
        "installed login shell required for product transport"
    );
    let login_body = std::fs::read_to_string(login).expect("login shell");
    assert!(!login_body.contains("DATABASE_URL="));
    assert!(!login_body.contains("postgres://"));
    if let Some(pass) = db_password_literal() {
        assert!(
            !login_body.contains(&pass),
            "login shell must not embed database password"
        );
    }

    let wrap = _f.sshd.run_dir.join("akc-wrap.sh");
    let wrap_body = std::fs::read_to_string(&wrap).expect("akc wrap");
    assert!(!wrap_body.contains("DATABASE_URL="));
    assert!(!wrap_body.contains("postgres://"));
    if let Some(pass) = db_password_literal() {
        assert!(
            !wrap_body.contains(&pass),
            "AKC wrapper must not embed database password"
        );
    }

    let runtime = _f.sshd.run_dir.join("runtime.env");
    assert!(runtime.exists(), "protected runtime.env required");
    let meta = std::fs::metadata(&runtime).expect("runtime.env meta");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode & 0o007,
        0,
        "runtime.env must not be world-accessible (mode={mode:o})"
    );
}

fn db_password_literal() -> Option<String> {
    let url = std::env::var("MUSEHUB_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;
    let rest = url.split("://").nth(1)?;
    let userpass = rest.split('@').next()?;
    let pass = userpass.split(':').nth(1)?;
    if pass.is_empty() || pass.len() < 8 {
        return None;
    }
    Some(pass.to_string())
}
