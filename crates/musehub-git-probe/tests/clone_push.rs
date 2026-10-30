use std::fs;

use musehub_git_probe::{probe, PROBE_BRANCH, PROBE_EMAIL, PROBE_FILE, PROBE_USER};

#[test]
fn clone_commit_push_round_trip() {
    let report = probe().expect("git probe must succeed with standard git");

    assert!(report.bare_repo.join("HEAD").is_file());
    assert!(report.work_tree.join(PROBE_FILE).is_file());
    let content = fs::read_to_string(report.work_tree.join(PROBE_FILE)).expect("probe file");
    assert!(!content.trim().is_empty());

    assert_eq!(report.commit.len(), 40, "commit: {}", report.commit);
    assert_eq!(report.tree.len(), 40, "tree: {}", report.tree);
    assert!(
        report
            .tree_entries
            .iter()
            .any(|entry| entry.ends_with(PROBE_FILE)),
        "tree entries: {:?}",
        report.tree_entries
    );
    assert_eq!(report.remote_head, format!("refs/heads/{PROBE_BRANCH}"));

    report.cleanup().expect("cleanup probe workspace");
}

#[test]
fn probe_identity_is_local_only() {
    assert_eq!(PROBE_EMAIL, "probe@musehub.local");
    assert_eq!(PROBE_USER, "musehub-probe");
    assert_eq!(PROBE_BRANCH, "main");
    assert_eq!(PROBE_FILE, "probe.txt");
}

#[test]
fn git_probe_manifest_version_matches_version_source() {
    assert_eq!(musehub_core::version(), env!("CARGO_PKG_VERSION"));
}
