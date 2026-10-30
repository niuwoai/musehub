//! Integration tests for the Colima OpenSSH forced-command probe.
//!
//! The temporary `sshd` runs inside the Linux VM. A missing VM, a missing
//! `sshd`, or a rejected command fails the test with the captured stderr.
//! Nothing is skipped, and no real Git pack is transferred.

use musehub_core::ErrorCode;
use musehub_ssh_probe::{probe, ProbeConfig, DEMO_UUID_PATH, SCOPE};

#[test]
fn forced_command_passes_server_identity_and_rejects_everything_else() {
    let report = probe(&ProbeConfig::default()).unwrap_or_else(|error| {
        panic!("ssh probe failed ({}): {error}", error.code())
    });

    assert_eq!(report.scope, SCOPE);
    assert_eq!(report.scope, "linux_openssh_colima");
    assert_eq!(report.accepted.exit_code, 0, "{:?}", report.accepted.stderr);
    assert_eq!(report.accepted.principal.as_deref(), Some("alice"));
    assert_eq!(report.accepted.command.as_deref(), Some("git-upload-pack"));
    assert_eq!(report.accepted.logical.as_deref(), Some("demo"));
    assert_eq!(report.accepted.mapped_path.as_deref(), Some(DEMO_UUID_PATH));
    assert!(
        !report.accepted.stdout.contains("mallory"),
        "client environment overrode the principal: {}",
        report.accepted.stdout
    );

    assert!(!report.rejected.is_empty());
    for rejected in &report.rejected {
        assert_ne!(rejected.exit_code, 0, "{rejected:?}");
        assert!(rejected.principal.is_none(), "{rejected:?}");
        assert!(
            rejected.stderr.contains("not allowed") || rejected.stderr.contains("missing"),
            "rejection must explain itself: {rejected:?}"
        );
    }

    assert_eq!(report.concurrent.len(), 2, "{:?}", report.concurrent);
    let mut principals: Vec<_> = report
        .concurrent
        .iter()
        .map(|session| {
            assert_eq!(session.exit_code, 0, "{:?}", session.stderr);
            session.principal.clone().expect("principal")
        })
        .collect();
    principals.sort();
    assert_eq!(principals, ["alice".to_string(), "bob".to_string()]);

    assert_ne!(report.cancelled.exit_code, 0, "{:?}", report.cancelled);
    assert!(
        !report.cancelled.stdout.contains("principal="),
        "cancelled session returned a result: {:?}",
        report.cancelled.stdout
    );
    assert!(
        !report.cancel_side_effect,
        "cancellation created a repository side effect"
    );
}
