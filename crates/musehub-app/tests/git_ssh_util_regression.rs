//! Regression tests for bounded ssh/git test helpers (no full product fixture required).

mod common;

use common::git_ssh_util::{
    classify_forward_denial, redact_test_output, run_bounded, ForwardDenialVerdict,
};
use std::process::Command;

#[test]
fn run_bounded_times_out_sleep() {
    let mut cmd = Command::new("sleep");
    cmd.arg("30");
    let out = run_bounded(cmd, 1);
    assert!(out.timed_out, "sleep 30 must hit 1s cap");
    assert!(!out.status.success());
}

#[test]
fn run_bounded_drains_large_stderr_without_deadlock() {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("yes x | head -c 262144 | cat >&2; exit 0");
    let out = run_bounded(cmd, 10);
    assert!(!out.timed_out);
    assert!(out.status.success());
    assert!(
        out.stderr.len() >= 200_000,
        "stderr drain too small: {}",
        out.stderr.len()
    );
}

#[test]
fn redact_strips_full_mhp_token() {
    let raw = "pat mhp_abc123XYZ-moretail leak";
    let red = redact_test_output(raw);
    assert!(!red.contains("mhp_abc123XYZ"));
    assert!(red.contains("[REDACTED_PAT]"));
}

#[test]
fn forward_verdict_rejects_timeout() {
    assert_eq!(
        classify_forward_denial(false, true, false, "administratively prohibited"),
        ForwardDenialVerdict::TimedOut
    );
}

#[test]
fn forward_verdict_rejects_tcp_channel_opened() {
    assert_eq!(
        classify_forward_denial(true, false, true, "administratively prohibited"),
        ForwardDenialVerdict::ChannelOpened
    );
}

#[test]
fn forward_verdict_rejects_unrelated_stderr() {
    assert_eq!(
        classify_forward_denial(true, false, false, "Warning: Permanently added host key"),
        ForwardDenialVerdict::Inconclusive
    );
}

#[test]
fn forward_verdict_accepts_admin_denial() {
    assert_eq!(
        classify_forward_denial(
            true,
            false,
            false,
            "channel 0: open failed: administratively prohibited: open failed"
        ),
        ForwardDenialVerdict::Denied
    );
}

#[test]
fn forward_verdict_positive_control_channel_open_fails_product_negative() {
    // Simulates AllowTcpForwarding yes: channel works, stderr unrelated — must not count as denial.
    let verdict = classify_forward_denial(true, false, true, "debug1: forwarding success");
    assert_ne!(verdict, ForwardDenialVerdict::Denied);
}
