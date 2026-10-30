use std::fs;

use musehub_agent_probe::{
    canceled_fixture_has_no_new_side_effect, fixture_commit, init_fixture_repository, parse_events,
    temporary_repository_path, verify_changed_files, TaskKind, Verification, FIXTURE_ADAPTER,
};
use musehub_core::ErrorCode;

struct TempRepo(std::path::PathBuf);

impl TempRepo {
    fn new() -> Self {
        let path = temporary_repository_path();
        init_fixture_repository(&path).expect("initialize fixture repository");
        Self(path)
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn events_accept_known_types_and_ignore_unknown_fields() {
    let input = r#"{"type":"started","trace":false}
{"type":"completed","kind":"analysis","unexpected":1}
"#;
    let report = parse_events(input, &[]).expect("events");

    assert_eq!(report.adapter, FIXTURE_ADAPTER);
    assert_eq!(report.events, vec!["started", "completed"]);
    assert_eq!(report.verification, Verification::AnalysisCompleted);
}

#[test]
fn invalid_json_and_unknown_event_type_are_rejected() {
    let invalid = parse_events("{]", &[]).expect_err("invalid JSON");
    assert_eq!(invalid.code(), "agent_probe_failed");
    assert!(invalid.to_string().contains("invalid JSON"));

    let unknown = parse_events(r#"{"type":"thinking"}"#, &[]).expect_err("unknown type");
    assert_eq!(unknown.code(), "agent_probe_failed");
    assert!(unknown.to_string().contains("unknown type"));
}

#[test]
fn implementation_requires_commit_and_non_empty_diff() {
    let missing = verify_changed_files(TaskKind::Implementation, None, &["src/lib.rs".to_string()])
        .expect("missing commit");
    assert!(matches!(missing, Verification::Rejected { .. }));

    let empty =
        verify_changed_files(TaskKind::Implementation, Some("abc123"), &[]).expect("empty diff");
    assert_eq!(
        empty,
        Verification::Rejected {
            reason: "implementation completion has an empty diff".to_string(),
        }
    );

    let analysis = verify_changed_files(TaskKind::Analysis, None, &[]).expect("analysis");
    assert_eq!(analysis, Verification::AnalysisCompleted);
    assert_ne!(
        analysis,
        Verification::VerificationRequested {
            commit: String::new()
        }
    );
}

#[test]
fn cancellation_creates_no_file_or_commit() {
    let repository = TempRepo::new();
    let report = canceled_fixture_has_no_new_side_effect(&repository.0).expect("cancel");

    assert_eq!(report.adapter, FIXTURE_ADAPTER);
    assert!(report.canceled);
    assert!(!report.marker_exists);
    assert!(report.git_status_clean);
}

#[test]
fn successful_fixture_commit_is_non_empty() {
    let repository = TempRepo::new();
    let commit = fixture_commit(&repository.0).expect("commit");
    let changed = vec!["change.txt".to_string()];
    let verification =
        verify_changed_files(TaskKind::Implementation, Some(&commit), &changed).expect("verify");

    assert_eq!(commit.len(), 40, "{commit}");
    assert_eq!(verification, Verification::VerificationRequested { commit });
    assert!(!fs::read_to_string(repository.0.join("change.txt"))
        .expect("change")
        .trim()
        .is_empty());
}
