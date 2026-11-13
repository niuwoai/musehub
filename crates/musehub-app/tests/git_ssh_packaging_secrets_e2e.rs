//! Product git-ssh packaging invariants: no DB literals in world-readable scripts.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Command;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::require_git_ssh_transport_prereqs;

#[tokio::test]
async fn git_ssh_public_scripts_contain_no_database_credentials() {
    require_git_ssh_transport_prereqs();
    let marker = format!("mh-pack-test-{}", uuid::Uuid::new_v4().simple());
    let synthetic_short_secret = "ci7probe";
    std::env::set_var("MUSEHUB_GIT_SSH_PACKAGING_TEST_MARKER", &marker);
    std::env::set_var(
        "MUSEHUB_GIT_SSH_PACKAGING_SYNTHETIC_SECRET",
        synthetic_short_secret,
    );
    let f = git_ssh_fixture("sshpack").await;
    let login = Path::new("/usr/local/lib/musehub/musehub-git-login.sh");
    assert!(
        login.exists(),
        "installed login shell required for product transport"
    );
    let login_body = std::fs::read_to_string(login).expect("login shell");
    assert!(!login_body.contains("DATABASE_URL="));
    assert!(!login_body.contains("postgres://"));
    assert!(
        !login_body.contains(&marker),
        "login shell must not embed packaging test marker"
    );

    let wrap = f.sshd.run_dir.join("akc-wrap.sh");
    let wrap_body = Command::new("sudo")
        .args(["cat", wrap.to_str().expect("wrap")])
        .output()
        .expect("sudo cat akc wrap")
        .stdout;
    let wrap_body = String::from_utf8(wrap_body).expect("utf8 akc wrap");
    assert!(!wrap_body.contains("DATABASE_URL="));
    assert!(!wrap_body.contains("postgres://"));
    assert!(!wrap_body.contains(&marker));

    let runtime = f.sshd.run_dir.join("runtime.env");
    assert!(
        Command::new("sudo")
            .args(["test", "-f", runtime.to_str().expect("runtime")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "protected runtime.env required — fail closed"
    );
    let runtime_body = String::from_utf8(
        Command::new("sudo")
            .args(["cat", runtime.to_str().expect("runtime")])
            .output()
            .expect("sudo cat runtime.env")
            .stdout,
    )
    .expect("utf8 runtime.env");
    assert!(
        runtime_body.contains(&marker),
        "marker must live only in protected runtime.env"
    );
    assert!(
        runtime_body.contains(synthetic_short_secret),
        "synthetic secret must be present in runtime.env for this test"
    );
    assert!(
        !login_body.contains(synthetic_short_secret),
        "short synthetic secret must not appear in login shell"
    );
    assert!(
        !wrap_body.contains(synthetic_short_secret),
        "short synthetic secret must not appear in akc wrap"
    );
    assert!(
        runtime_body.contains("DATABASE_URL="),
        "runtime.env must carry credentials for gateway"
    );
    assert!(
        runtime_body.contains("MUSEHUB_GIT_STORAGE_GROUP='musehub-git'")
            || runtime_body.contains("MUSEHUB_GIT_STORAGE_GROUP=musehub-git"),
        "runtime.env must export storage group for login-shell gateway sessions"
    );

    let rmode_line = String::from_utf8(
        Command::new("sudo")
            .args(["stat", "-c", "%a", runtime.to_str().expect("runtime")])
            .output()
            .expect("stat runtime.env")
            .stdout,
    )
    .unwrap();
    let mode = u32::from_str_radix(rmode_line.trim(), 8).expect("octal mode");
    assert_eq!(mode, 0o640, "runtime.env mode 0640");
    assert_eq!(mode & 0o007, 0, "runtime.env must not be world-accessible");

    let url = std::env::var("MUSEHUB_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("DATABASE_URL required for packaging secrets test");
    if let Some(pass) = db_password_literal(&url) {
        assert!(
            !login_body.contains(&pass),
            "login shell must not embed database password literal"
        );
        assert!(
            !wrap_body.contains(&pass),
            "akc wrap must not embed database password literal"
        );
    }
}

fn db_password_literal(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let userpass = rest.split('@').next()?;
    let pass = userpass.split(':').nth(1)?;
    if pass.is_empty() || pass.len() < 12 {
        return None;
    }
    Some(pass.to_string())
}
