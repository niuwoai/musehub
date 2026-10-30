//! Local fixture for the Agent event contract.
//!
//! `FIXTURE_ADAPTER` identifies every report. This crate does not call a model,
//! third-party CLI, or MCP server, and it is not a selected production adapter.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use musehub_core::ErrorCode;
use serde::Deserialize;
use serde_json::Value;

/// Adapter identity that prevents fixture evidence being reported as production.
pub const FIXTURE_ADAPTER: &str = "fixture";

const EVENT_TYPES: [&str; 7] = [
    "started",
    "progress",
    "needs_input",
    "usage",
    "artifact",
    "completed",
    "error",
];

/// Task kinds accepted by the fixture contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Implementation,
    Analysis,
}

impl TaskKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Analysis => "analysis",
        }
    }
}

/// Platform result after checking an Agent completion claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// The fixture requested verification and the platform found a non-empty change.
    VerificationRequested { commit: String },
    /// Analysis completed without claiming that code was implemented.
    AnalysisCompleted,
    /// The completion claim did not satisfy the contract.
    Rejected { reason: String },
}

/// Parsed fixture event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventReport {
    pub adapter: &'static str,
    pub events: Vec<String>,
    pub verification: Verification,
}

/// Result of a fixture process whose cancellation must have no new side effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationReport {
    pub adapter: &'static str,
    pub canceled: bool,
    pub marker_exists: bool,
    pub git_status_clean: bool,
}

/// Parses NDJSON events and verifies a completion claim against `changed_files`.
pub fn parse_events(input: &str, changed_files: &[String]) -> Result<EventReport, AgentProbeError> {
    let mut events = Vec::new();
    let mut verification = None;

    for (index, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|error| {
            AgentProbeError::failed(format!("event line {} is invalid JSON: {error}", index + 1))
        })?;
        let event: Event = serde_json::from_value(value).map_err(|error| {
            AgentProbeError::failed(format!("event line {} is invalid: {error}", index + 1))
        })?;
        if !EVENT_TYPES.contains(&event.event_type.as_str()) {
            return Err(AgentProbeError::failed(format!(
                "event line {} has unknown type {:?}",
                index + 1,
                event.event_type
            )));
        }
        if event.event_type == "completed" {
            verification = Some(verify_completion(
                event.kind.as_deref(),
                event.commit.as_deref(),
                changed_files,
            )?);
        }
        events.push(event.event_type);
    }

    Ok(EventReport {
        adapter: FIXTURE_ADAPTER,
        events,
        verification: verification.unwrap_or(Verification::Rejected {
            reason: "fixture stream contains no completed event".to_string(),
        }),
    })
}

/// Checks whether a completion claim matches the files actually changed.
pub fn verify_changed_files(
    kind: TaskKind,
    commit: Option<&str>,
    changed_files: &[String],
) -> Result<Verification, AgentProbeError> {
    verify_completion(Some(kind.as_str()), commit, changed_files)
}

fn verify_completion(
    kind: Option<&str>,
    commit: Option<&str>,
    changed_files: &[String],
) -> Result<Verification, AgentProbeError> {
    let kind = match kind.unwrap_or("") {
        "implementation" => TaskKind::Implementation,
        "analysis" => TaskKind::Analysis,
        other => {
            return Err(AgentProbeError::failed(format!(
                "completed event has unknown kind {other:?}"
            )))
        }
    };
    match kind {
        TaskKind::Analysis => Ok(Verification::AnalysisCompleted),
        TaskKind::Implementation => {
            let commit = commit.unwrap_or("").trim();
            if commit.is_empty() {
                return Ok(Verification::Rejected {
                    reason: "implementation completion requires a non-empty commit".to_string(),
                });
            }
            if changed_files.iter().all(|file| file.trim().is_empty()) {
                return Ok(Verification::Rejected {
                    reason: "implementation completion has an empty diff".to_string(),
                });
            }
            Ok(Verification::VerificationRequested {
                commit: commit.to_string(),
            })
        }
    }
}

/// Runs a local shell fixture that would write after a delay, then cancels it.
///
/// The marker and Git status are checked after the process exits. No new commit is created.
pub fn canceled_fixture_has_no_new_side_effect(
    repository: &Path,
) -> Result<CancellationReport, AgentProbeError> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("sleep 30; printf late > marker.txt")
        .current_dir(repository)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| AgentProbeError::io("spawn cancel fixture", source))?;

    thread::sleep(Duration::from_millis(50));
    child
        .kill()
        .map_err(|source| AgentProbeError::io("cancel fixture", source))?;
    let status = child
        .wait()
        .map_err(|source| AgentProbeError::io("wait for canceled fixture", source))?;
    thread::sleep(Duration::from_millis(100));

    Ok(CancellationReport {
        adapter: FIXTURE_ADAPTER,
        canceled: !status.success(),
        marker_exists: repository.join("marker.txt").exists(),
        git_status_clean: git_status(repository)?.trim().is_empty(),
    })
}

/// Writes one non-empty file and records one local commit with a command-scoped identity.
pub fn fixture_commit(repository: &Path) -> Result<String, AgentProbeError> {
    let marker = repository.join("change.txt");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&marker)
        .map_err(|source| AgentProbeError::io("open fixture change", source))?;
    writeln!(file, "fixture change")
        .map_err(|source| AgentProbeError::io("write fixture change", source))?;

    git(repository, &["add", "--", "change.txt"])?;
    git(
        repository,
        &[
            "-c",
            "user.email=probe@musehub.local",
            "-c",
            "user.name=musehub-probe",
            "commit",
            "--no-gpg-sign",
            "-m",
            "fixture: non-empty change",
        ],
    )?;
    git(repository, &["rev-parse", "HEAD"])
}

/// Creates an empty temporary Git repository using only command arguments.
pub fn init_fixture_repository(repository: &Path) -> Result<(), AgentProbeError> {
    fs::create_dir_all(repository)
        .map_err(|source| AgentProbeError::io("create fixture repository", source))?;
    git(repository, &["init", "--quiet"])?;
    Ok(())
}

fn git(repository: &Path, args: &[&str]) -> Result<String, AgentProbeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|source| AgentProbeError::io("run git", source))?;
    if !output.status.success() {
        return Err(AgentProbeError::failed(format!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_status(repository: &Path) -> Result<String, AgentProbeError> {
    git(repository, &["status", "--porcelain"])
}

#[derive(Debug, Deserialize)]
struct Event {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    commit: Option<String>,
}

/// Fixture contract failures. Every variant uses `agent_probe_failed`.
#[derive(Debug, thiserror::Error)]
pub enum AgentProbeError {
    #[error("agent probe failed: {detail}")]
    Failed { detail: String },
    #[error("agent probe step `{step}` could not run: {source}")]
    Io {
        step: &'static str,
        #[source]
        source: io::Error,
    },
}

impl AgentProbeError {
    fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
        }
    }

    fn io(step: &'static str, source: io::Error) -> Self {
        Self::Io { step, source }
    }
}

impl ErrorCode for AgentProbeError {
    fn code(&self) -> &'static str {
        "agent_probe_failed"
    }
}

/// Builds a unique path under the system temporary directory.
pub fn temporary_repository_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("musehub-agent-{}-{nanos}", std::process::id()))
}
