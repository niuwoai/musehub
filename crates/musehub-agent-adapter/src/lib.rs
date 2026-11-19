//! Agent adapter contract: spawn a headless CLI, stream NDJSON events, enforce budget fields.
//!
//! | Kind | Meaning |
//! |---|---|
//! | `reference` | In-repo deterministic CLI; **not** a production model adapter |
//! | `codex` | OpenAI Codex CLI when configured; **live model tasks require user credentials** |

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use musehub_core::ErrorCode;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Instant};

/// Reference adapter identity (deterministic, no model).
pub const REFERENCE_ADAPTER: &str = "reference";

/// Production target adapter name (requires user credentials when live).
pub const CODEX_ADAPTER: &str = "codex";

/// Explicit deterministic sim binary (not a live model).
pub const CODEX_SIM_ADAPTER: &str = "codex-sim";

/// Task handed to an adapter process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTask {
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub branch: Option<String>,
}

/// Parsed usage snapshot from adapter stream.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub tokens: u64,
    pub microusd: u64,
    pub budget_unknown: bool,
    #[serde(default)]
    pub microusd_unknown: bool,
    /// Minimal provider-reported usage object; excludes prompts and conversation output.
    #[serde(default)]
    pub evidence: Option<serde_json::Value>,
}

/// Outcome of a completed adapter run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Succeeded {
        commit: Option<String>,
        usage: UsageTotals,
    },
    Failed {
        code: String,
        detail: String,
        usage: Option<UsageTotals>,
    },
    Canceled {
        usage: UsageTotals,
    },
}

/// Active adapter child process.
pub struct AdapterRun {
    child: tokio::process::Child,
    canceled: Arc<AtomicBool>,
    protocol: AdapterProtocol,
    work_dir: PathBuf,
    initial_commit: Option<String>,
    expected_branch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterProtocol {
    MuseHubNdjson,
    CodexExecJsonl,
}

impl AdapterRun {
    pub fn child_id(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn request_cancel(&mut self) {
        self.canceled.store(true, Ordering::SeqCst);
        if let Some(pid) = self.child.id() {
            kill_process_group(pid);
        }
        let _ = self.child.start_kill();
    }
}

#[cfg(unix)]
pub fn kill_process_group(pid: u32) {
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub fn kill_process_group(_pid: u32) {}

#[cfg(unix)]
fn configure_process_group(cmd: &mut TokioCommand) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.as_std_mut().pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_cmd: &mut TokioCommand) {}

/// Puts the child in its own process group so cancel can kill the whole tree.
pub fn configure_child_process_group(cmd: &mut TokioCommand) {
    configure_process_group(cmd);
}

/// Spawns the reference adapter binary in `work_dir`.
pub async fn spawn_reference(
    task: &AgentTask,
    work_dir: &Path,
    binary: &Path,
) -> Result<AdapterRun, AdapterError> {
    spawn_ndjson_adapter(binary, &[], task, work_dir, REFERENCE_ADAPTER).await
}

/// Spawns the simulated Codex CLI (CI / no credentials).
pub async fn spawn_codex_sim(
    task: &AgentTask,
    work_dir: &Path,
    binary: &Path,
) -> Result<AdapterRun, AdapterError> {
    spawn_ndjson_adapter(binary, &[], task, work_dir, CODEX_ADAPTER).await
}

fn codex_reasoning_effort_config() -> Result<Option<String>, AdapterError> {
    let Ok(value) = std::env::var("MUSEHUB_AGENT_CODEX_REASONING_EFFORT") else {
        return Ok(None);
    };
    let effort = match value.as_str() {
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "xhigh" => "xhigh",
        _ => {
            return Err(AdapterError::Failed {
                detail: "MUSEHUB_AGENT_CODEX_REASONING_EFFORT must be low, medium, high, or xhigh"
                    .into(),
            });
        }
    };
    Ok(Some(format!("model_reasoning_effort=\"{effort}\"")))
}

fn codex_exec_args(
    reasoning_config: Option<&str>,
    git_dir: Option<&Path>,
    has_initial_commit: bool,
) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--config".to_string(),
        "approval_policy=\"never\"".to_string(),
        "--config".to_string(),
        "sandbox_mode=\"workspace-write\"".to_string(),
        "--ignore-user-config".to_string(),
        "--ephemeral".to_string(),
    ];
    if let Some(git_dir) = git_dir {
        args.push("--add-dir".to_string());
        args.push(git_dir.to_string_lossy().into_owned());
    }
    if let Some(config) = reasoning_config {
        args.extend(["--config".to_string(), config.to_string()]);
    }
    if !has_initial_commit {
        args.push("--skip-git-repo-check".to_string());
    }
    args.push("-".to_string());
    args
}

/// Spawns Codex CLI when `MUSEHUB_AGENT_CODEX_BIN` is set (live path — not verified without credentials).
pub async fn spawn_codex(task: &AgentTask, work_dir: &Path) -> Result<AdapterRun, AdapterError> {
    let binary =
        std::env::var("MUSEHUB_AGENT_CODEX_BIN").map_err(|_| AdapterError::Unavailable {
            adapter: CODEX_ADAPTER.to_string(),
            reason: "set MUSEHUB_AGENT_CODEX_BIN to the codex executable".to_string(),
        })?;
    let has_initial_commit = git_output(work_dir, &["rev-parse", "HEAD"]).is_some();
    let prompt = if has_initial_commit {
        format!(
            "You are implementing a MuseHub AgentRun in branch `{}`.\n\nTask:\n{}\n\nMake the requested changes in the current workspace. Run appropriate local checks. Commit the completed changes on the current branch with a concise commit message. Do not modify files outside this workspace. If you cannot complete the task safely, explain why and exit unsuccessfully.",
            task.branch.as_deref().unwrap_or("the current branch"),
            task.message
        )
    } else {
        format!(
            "You are completing a MuseHub task in a temporary workspace without Git history.\n\nTask:\n{}\n\nMake the requested changes in the current workspace and run appropriate local checks. Do not initialize Git or modify files outside this workspace. If you cannot complete the task safely, explain why and exit unsuccessfully.",
            task.message
        )
    };
    let reasoning_config = codex_reasoning_effort_config()?;
    let git_dir = if has_initial_commit {
        Some(
            git_output(work_dir, &["rev-parse", "--absolute-git-dir"])
                .map(PathBuf::from)
                .ok_or_else(|| AdapterError::Failed {
                    detail: "unable to resolve AgentRun Git metadata directory".to_string(),
                })?,
        )
    } else {
        None
    };
    let args = codex_exec_args(
        reasoning_config.as_deref(),
        git_dir.as_deref(),
        has_initial_commit,
    );
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    spawn_process(
        Path::new(&binary),
        &args,
        &prompt,
        work_dir,
        CODEX_ADAPTER,
        AdapterProtocol::CodexExecJsonl,
        task.branch.clone(),
    )
    .await
}

async fn spawn_ndjson_adapter(
    binary: &Path,
    extra_args: &[&str],
    task: &AgentTask,
    work_dir: &Path,
    adapter_name: &str,
) -> Result<AdapterRun, AdapterError> {
    let payload = serde_json::to_string(task).map_err(|error| AdapterError::Failed {
        detail: error.to_string(),
    })?;
    spawn_process(
        binary,
        extra_args,
        &format!("{payload}\n"),
        work_dir,
        adapter_name,
        AdapterProtocol::MuseHubNdjson,
        None,
    )
    .await
}

async fn spawn_process(
    binary: &Path,
    extra_args: &[&str],
    stdin_payload: &str,
    work_dir: &Path,
    adapter_name: &str,
    protocol: AdapterProtocol,
    expected_branch: Option<String>,
) -> Result<AdapterRun, AdapterError> {
    if !binary.is_file() {
        return Err(AdapterError::Unavailable {
            adapter: adapter_name.to_string(),
            reason: format!("missing executable {}", binary.display()),
        });
    }
    let initial_commit = if protocol == AdapterProtocol::CodexExecJsonl {
        git_output(work_dir, &["rev-parse", "HEAD"])
    } else {
        None
    };
    let mut command = TokioCommand::new(binary);
    if protocol == AdapterProtocol::CodexExecJsonl {
        command.env_clear();
        for key in [
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "TMP",
            "TEMP",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TERM",
            "CODEX_HOME",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
    }
    command
        .args(extra_args)
        .current_dir(work_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(|source| AdapterError::Io {
        step: "spawn adapter",
        source,
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_payload.as_bytes())
            .await
            .map_err(|source| AdapterError::Io {
                step: "write adapter task",
                source,
            })?;
    }
    Ok(AdapterRun {
        child,
        canceled: Arc::new(AtomicBool::new(false)),
        protocol,
        work_dir: work_dir.to_path_buf(),
        initial_commit,
        expected_branch,
    })
}

fn git_output(work_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(work_dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_worktree_has_changes(work_dir: &Path) -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(work_dir)
        .output()
        .ok()?;
    output.status.success().then_some(!output.stdout.is_empty())
}

/// Waits for the child, parses NDJSON from stdout, returns outcome.
pub async fn finish_run(
    mut run: AdapterRun,
    deadline: Duration,
    budget_tokens: Option<u64>,
    budget_microusd: Option<u64>,
    cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<RunOutcome, AdapterError> {
    let started = Instant::now();
    let stderr_task = run.child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            const CAPTURE_LIMIT: usize = 8192;
            let mut captured = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let remaining = CAPTURE_LIMIT.saturating_sub(captured.len());
                captured.extend_from_slice(&chunk[..read.min(remaining)]);
            }
            captured
        })
    });
    let mut stdout = run.child.stdout.take().ok_or(AdapterError::Failed {
        detail: "adapter stdout missing".to_string(),
    })?;
    let mut buffer = Vec::new();
    let mut usage_total = UsageTotals {
        budget_unknown: run.protocol == AdapterProtocol::CodexExecJsonl,
        microusd_unknown: run.protocol == AdapterProtocol::CodexExecJsonl,
        ..UsageTotals::default()
    };
    let mut reader = tokio::io::BufReader::new(&mut stdout);
    loop {
        if cancel_rx
            .as_ref()
            .is_some_and(|receiver| *receiver.borrow())
        {
            run.request_cancel();
        }
        if started.elapsed() > deadline {
            run.request_cancel();
            return Ok(RunOutcome::Failed {
                code: "agent_timeout".to_string(),
                detail: format!("exceeded {:?}", deadline),
                usage: Some(usage_total),
            });
        }
        let mut line = String::new();
        match timeout(Duration::from_millis(250), reader.read_line(&mut line)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(_)) => {
                buffer.extend_from_slice(line.as_bytes());
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                    if let Some(usage) = parse_usage_event(&event, run.protocol)? {
                        // Codex `turn.completed.usage` reports cumulative session totals,
                        // so keep the newest snapshot instead of summing each completed turn.
                        usage_total = usage;
                        if budget_microusd.is_some() && usage_total.microusd_unknown {
                            run.request_cancel();
                            return Ok(RunOutcome::Failed {
                                code: "agent_budget_unknown".to_string(),
                                detail: "monetary budget limit set but adapter cannot report cost"
                                    .to_string(),
                                usage: Some(usage_total),
                            });
                        }
                        if budget_exceeded(&usage_total, budget_tokens, budget_microusd) {
                            run.request_cancel();
                            return Ok(RunOutcome::Failed {
                                code: "agent_budget_exceeded".to_string(),
                                detail: "reported usage exceeded the configured limit after a usage event".to_string(),
                                usage: Some(usage_total),
                            });
                        }
                    }
                }
            }
            Ok(Err(source)) => {
                return Err(AdapterError::Io {
                    step: "read adapter stdout",
                    source,
                });
            }
            Err(_) if run.child.try_wait().ok().flatten().is_some() => break,
            Err(_) => {}
        }
    }
    let status = run.child.wait().await.map_err(|source| AdapterError::Io {
        step: "wait adapter",
        source,
    })?;
    let stderr_category = match stderr_task {
        Some(task) => match task.await {
            Ok(stderr) => codex_stderr_category(&stderr),
            Err(_) => "read_failed",
        },
        None => "unavailable",
    };
    drain_remaining_stdout(&mut reader, &mut buffer).await?;
    let stdout_error_category = codex_jsonl_error_category(&buffer);
    let final_usage = parse_stream_usage(&buffer, run.protocol)?;
    if budget_exceeded(&final_usage, budget_tokens, budget_microusd) {
        return Ok(RunOutcome::Failed {
            code: "agent_budget_exceeded".to_string(),
            detail: "reported usage exceeded the configured limit after a usage event".to_string(),
            usage: Some(final_usage),
        });
    }
    if run.canceled.load(Ordering::SeqCst) {
        return Ok(RunOutcome::Canceled { usage: final_usage });
    }
    if !status.success() {
        return Ok(RunOutcome::Failed {
            code: "agent_adapter_failed".to_string(),
            detail: if run.protocol == AdapterProtocol::CodexExecJsonl {
                let category = stdout_error_category.unwrap_or(stderr_category);
                format!("exit {}; diagnostic_category={category}", status)
            } else {
                format!("exit {}", status)
            },
            usage: Some(final_usage),
        });
    }
    let summary = parse_stream_summary(&buffer, run.protocol)?;
    if (budget_tokens.is_some() && summary.usage.budget_unknown)
        || (budget_microusd.is_some() && summary.usage.microusd_unknown)
    {
        return Ok(RunOutcome::Failed {
            code: "agent_budget_unknown".to_string(),
            detail: "configured budget limit has no corresponding usage measurement".to_string(),
            usage: Some(summary.usage.clone()),
        });
    }
    let commit = if run.protocol == AdapterProtocol::CodexExecJsonl {
        if !summary.completed {
            return Ok(RunOutcome::Failed {
                code: "agent_adapter_incomplete".to_string(),
                detail: "Codex exited successfully without a completed turn event".to_string(),
                usage: Some(summary.usage.clone()),
            });
        }
        let current = git_output(&run.work_dir, &["rev-parse", "HEAD"]);
        if let Some(initial) = run.initial_commit.as_deref() {
            let Some(current_commit) = current.as_deref() else {
                return Ok(RunOutcome::Failed {
                    code: "agent_head_unreadable".to_string(),
                    detail: "Codex completed without a readable HEAD commit".to_string(),
                    usage: Some(summary.usage.clone()),
                });
            };
            if current_commit == initial {
                let (code, detail) = match git_worktree_has_changes(&run.work_dir) {
                    Some(true) => (
                        "agent_changes_uncommitted",
                        "Codex completed with worktree changes but without creating a commit",
                    ),
                    Some(false) => (
                        "agent_commit_missing",
                        "Codex completed without creating changes or a new commit",
                    ),
                    None => (
                        "agent_worktree_status_unavailable",
                        "Codex completed without a new commit and Git status could not be read",
                    ),
                };
                return Ok(RunOutcome::Failed {
                    code: code.to_string(),
                    detail: detail.to_string(),
                    usage: Some(summary.usage.clone()),
                });
            }
        }
        if let Some(expected) = run.expected_branch.as_deref() {
            let branch = git_output(&run.work_dir, &["branch", "--show-current"]);
            if branch.as_deref() != Some(expected) {
                return Ok(RunOutcome::Failed {
                    code: "agent_branch_changed".to_string(),
                    detail: "Codex changed the assigned work branch".to_string(),
                    usage: Some(summary.usage.clone()),
                });
            }
        }
        current
    } else {
        summary.commit
    };
    Ok(RunOutcome::Succeeded {
        commit,
        usage: summary.usage,
    })
}

async fn drain_remaining_stdout(
    reader: &mut tokio::io::BufReader<impl tokio::io::AsyncRead + Unpin>,
    buffer: &mut Vec<u8>,
) -> Result<(), AdapterError> {
    let mut line = String::new();
    while reader
        .read_line(&mut line)
        .await
        .map_err(|source| AdapterError::Io {
            step: "read adapter stdout",
            source,
        })?
        > 0
    {
        buffer.extend_from_slice(line.as_bytes());
        line.clear();
    }
    Ok(())
}

fn budget_exceeded(
    usage: &UsageTotals,
    budget_tokens: Option<u64>,
    budget_microusd: Option<u64>,
) -> bool {
    if let Some(limit) = budget_tokens {
        if !usage.budget_unknown && usage.tokens > limit {
            return true;
        }
    }
    if let Some(limit) = budget_microusd {
        if usage.microusd > limit {
            return true;
        }
    }
    false
}

pub fn parse_events(input: &str) -> Result<Vec<AdapterEvent>, AdapterError> {
    let mut events = Vec::new();
    for (index, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|error| AdapterError::Failed {
                detail: format!("line {}: {error}", index + 1),
            })?;
        let event: AdapterEvent =
            serde_json::from_value(value).map_err(|error| AdapterError::Failed {
                detail: format!("line {}: {error}", index + 1),
            })?;
        events.push(event);
    }
    Ok(events)
}

#[derive(Debug, Clone, Deserialize)]
struct StreamSummary {
    commit: Option<String>,
    usage: UsageTotals,
    completed: bool,
}

fn parse_stream_summary(
    buffer: &[u8],
    protocol: AdapterProtocol,
) -> Result<StreamSummary, AdapterError> {
    let text = String::from_utf8_lossy(buffer);
    let mut commit = None;
    let mut usage = UsageTotals {
        budget_unknown: protocol == AdapterProtocol::CodexExecJsonl,
        microusd_unknown: protocol == AdapterProtocol::CodexExecJsonl,
        ..UsageTotals::default()
    };
    let mut completed = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|error| AdapterError::Failed {
                detail: error.to_string(),
            })?;
        let event_type = value.get("type").and_then(|v| v.as_str());
        if protocol == AdapterProtocol::MuseHubNdjson && event_type == Some("completed") {
            completed = true;
            commit = value
                .get("commit")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
        }
        if let Some(event_usage) = parse_usage_event(&value, protocol)? {
            // Codex turn usage is a cumulative session snapshot; use the latest one.
            usage = event_usage;
        }
        if protocol == AdapterProtocol::CodexExecJsonl && event_type == Some("turn.completed") {
            completed = true;
        }
    }
    Ok(StreamSummary {
        commit,
        usage,
        completed,
    })
}

fn parse_usage_event(
    value: &serde_json::Value,
    protocol: AdapterProtocol,
) -> Result<Option<UsageTotals>, AdapterError> {
    let event_type = value.get("type").and_then(|v| v.as_str());
    match protocol {
        AdapterProtocol::MuseHubNdjson if event_type == Some("usage") => {
            parse_usage_line(value).map(Some)
        }
        AdapterProtocol::CodexExecJsonl if event_type == Some("turn.completed") => {
            let Some(usage) = value.get("usage") else {
                return Ok(None);
            };
            let tokens = usage
                .get("total_tokens")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    Some(
                        usage
                            .get("input_tokens")?
                            .as_u64()?
                            .saturating_add(usage.get("output_tokens")?.as_u64()?),
                    )
                });
            Ok(tokens.map(|tokens| UsageTotals {
                tokens,
                microusd: 0,
                budget_unknown: false,
                microusd_unknown: true,
                evidence: Some(serde_json::json!({
                    "protocol": "codex_exec_jsonl",
                    "event_type": "turn.completed",
                    "usage": usage,
                })),
            }))
        }
        _ => Ok(None),
    }
}

fn parse_stream_usage(
    buffer: &[u8],
    protocol: AdapterProtocol,
) -> Result<UsageTotals, AdapterError> {
    let text = String::from_utf8_lossy(buffer);
    let mut usage = UsageTotals {
        budget_unknown: protocol == AdapterProtocol::CodexExecJsonl,
        microusd_unknown: protocol == AdapterProtocol::CodexExecJsonl,
        ..UsageTotals::default()
    };
    for line in text.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            if let Some(event_usage) = parse_usage_event(&value, protocol)? {
                usage = event_usage;
            }
        }
    }
    Ok(usage)
}

fn parse_usage_line(value: &serde_json::Value) -> Result<UsageTotals, AdapterError> {
    let tokens = value.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let microusd = value.get("microusd").and_then(|v| v.as_u64()).unwrap_or(0);
    let budget_unknown = value
        .get("budget_unknown")
        .and_then(|v| v.as_bool())
        .unwrap_or(tokens == 0 && microusd == 0);
    let microusd_unknown = value
        .get("microusd_unknown")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| value.get("microusd").and_then(|v| v.as_u64()).is_none());
    Ok(UsageTotals {
        tokens,
        microusd,
        budget_unknown,
        microusd_unknown,
        evidence: None,
    })
}

fn codex_jsonl_error_category(stdout: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let event_type = event.get("type").and_then(serde_json::Value::as_str);
        let error = event.get("error");
        if event_type.is_some_and(|kind| kind == "error" || kind.ends_with(".failed"))
            || error.is_some()
        {
            let message = event
                .get("message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    error.and_then(|value| value.get("message").and_then(serde_json::Value::as_str))
                });
            return Some(message.map_or("codex_error_event", |message| {
                codex_stderr_category(message.as_bytes())
            }));
        }
    }
    None
}

fn codex_stderr_category(stderr: &[u8]) -> &'static str {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if message.trim().is_empty() {
        "empty"
    } else if [
        "unknown argument",
        "unexpected argument",
        "unrecognized option",
    ]
    .iter()
    .any(|marker| message.contains(marker))
    {
        "cli_arguments"
    } else if ["401", "unauthorized", "authentication", "not logged in"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "authentication"
    } else if ["429", "rate limit", "too many requests"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "rate_limit"
    } else if ["approval", "auto-review", "automatic review"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "approval"
    } else if [
        "invalid configuration",
        "failed to parse config",
        "config file",
        "model_reasoning_effort",
    ]
    .iter()
    .any(|marker| message.contains(marker))
    {
        "configuration"
    } else if message.contains("sandbox") {
        "sandbox"
    } else if message.contains("trust") {
        "trust"
    } else if ["network", "dns", "connect", "timeout", "tls"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "network"
    } else if ["cannot be used", "conflicts with", "only valid with"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "option_conflict"
    } else if message.contains("requires") {
        "option_requirement"
    } else if message.contains("error") {
        "cli_error"
    } else {
        "unclassified"
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AdapterEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("agent adapter unavailable ({adapter}): {reason}")]
    Unavailable { adapter: String, reason: String },
    #[error("agent adapter failed: {detail}")]
    Failed { detail: String },
    #[error("agent adapter step `{step}` failed: {source}")]
    Io {
        step: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl ErrorCode for AdapterError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "agent_adapter_unavailable",
            Self::Failed { .. } | Self::Io { .. } => "agent_adapter_failed",
        }
    }
}

pub fn drain_stderr_limited(child: &mut Child, cap: usize) -> Result<String, AdapterError> {
    let stderr = child.stderr.take();
    let Some(mut stderr) = stderr else {
        return Ok(String::new());
    };
    let mut reader = BufReader::new(&mut stderr);
    let mut out = String::new();
    let mut line = String::new();
    while reader
        .read_line(&mut line)
        .map_err(|source| AdapterError::Io {
            step: "read adapter stderr",
            source,
        })?
        > 0
    {
        if out.len() + line.len() > cap {
            break;
        }
        out.push_str(&line);
        line.clear();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_unknown_when_not_reported() {
        let value = serde_json::json!({"type":"usage"});
        let usage = parse_usage_line(&value).expect("parse");
        assert!(usage.budget_unknown);
    }

    #[test]
    fn budget_limit_flags_tokens() {
        let value =
            serde_json::json!({"type":"usage","tokens":99,"microusd":0,"budget_unknown":false});
        let usage = parse_usage_line(&value).expect("parse");
        assert!(!usage.budget_unknown);
        assert_eq!(usage.tokens, 99);
    }

    #[test]
    fn codex_usage_uses_latest_cumulative_snapshot() {
        let input = r#"{"type":"turn.completed","usage":{"total_tokens":40}}
{"type":"turn.completed","usage":{"total_tokens":101}}"#;
        let usage = parse_stream_usage(input.as_bytes(), AdapterProtocol::CodexExecJsonl)
            .expect("parse Codex usage");
        assert_eq!(usage.tokens, 101);
        assert!(!usage.budget_unknown);
        assert!(usage.microusd_unknown);
        assert_eq!(
            usage.evidence,
            Some(serde_json::json!({
                "protocol": "codex_exec_jsonl",
                "event_type": "turn.completed",
                "usage": { "total_tokens": 101 },
            }))
        );
    }

    #[test]
    fn codex_usage_falls_back_to_input_and_output_sum() {
        let input = r#"{"type":"turn.completed","usage":{"input_tokens":80,"output_tokens":21}}"#;
        let usage = parse_stream_usage(input.as_bytes(), AdapterProtocol::CodexExecJsonl)
            .expect("parse Codex usage");
        assert_eq!(usage.tokens, 101);
    }

    #[test]
    fn final_cumulative_usage_snapshot_is_checked_against_budget() {
        let input = r#"{"type":"turn.completed","usage":{"total_tokens":4097}}"#;
        let usage = parse_stream_usage(input.as_bytes(), AdapterProtocol::CodexExecJsonl)
            .expect("parse Codex usage");
        assert!(budget_exceeded(&usage, Some(4096), None));
    }

    #[test]
    fn final_usage_parse_ignores_non_json_output_lines() {
        let input =
            "adapter diagnostic\n{\"type\":\"turn.completed\",\"usage\":{\"total_tokens\":17}}\n";
        let usage = parse_stream_usage(input.as_bytes(), AdapterProtocol::CodexExecJsonl)
            .expect("parse Codex usage");
        assert_eq!(usage.tokens, 17);
    }

    #[test]
    fn codex_exec_disables_approval_prompts_with_workspace_sandbox() {
        let args = codex_exec_args(
            Some("model_reasoning_effort=\"low\""),
            Some(Path::new("/tmp/musehub-agent/.git")),
            true,
        );
        assert!(!args.iter().any(|arg| arg == "--approve-for-me"));
        assert!(args.iter().any(|arg| arg == "approval_policy=\"never\""));
        assert!(args
            .iter()
            .any(|arg| arg == "sandbox_mode=\"workspace-write\""));
        assert!(args.iter().any(|arg| arg == "--ignore-user-config"));
        assert!(!args.iter().any(|arg| arg == "--sandbox"));
        assert!(args.iter().any(|arg| arg == "--ephemeral"));
        assert!(args
            .iter()
            .any(|arg| arg == "model_reasoning_effort=\"low\""));
        let add_dir = args
            .iter()
            .position(|arg| arg == "--add-dir")
            .expect("Git metadata grant");
        assert_eq!(
            args.get(add_dir + 1).map(String::as_str),
            Some("/tmp/musehub-agent/.git")
        );
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn codex_diagnostics_prioritize_sandbox_and_trust_signals() {
        assert_eq!(
            codex_stderr_category(b"workspace requires sandbox mode"),
            "sandbox"
        );
        assert_eq!(codex_stderr_category(b"project requires trust"), "trust");
        assert_eq!(
            codex_stderr_category(b"this option requires another option"),
            "option_requirement"
        );
    }

    #[test]
    fn parse_completed_commit() {
        let input = r#"{"type":"started"}
{"type":"completed","kind":"implementation","commit":"abc"}
"#;
        let summary = parse_stream_summary(input.as_bytes(), AdapterProtocol::MuseHubNdjson)
            .expect("summary");
        assert_eq!(summary.commit.as_deref(), Some("abc"));
    }
}
