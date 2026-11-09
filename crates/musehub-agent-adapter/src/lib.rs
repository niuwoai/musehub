//! Agent adapter contract: spawn a headless CLI, stream NDJSON events, enforce budget fields.
//!
//! | Kind | Meaning |
//! |---|---|
//! | `reference` | In-repo deterministic CLI; **not** a production model adapter |
//! | `codex` | OpenAI Codex CLI when configured; **live model tasks require user credentials** |

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use musehub_core::ErrorCode;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Instant};

/// Reference adapter identity (deterministic, no model).
pub const REFERENCE_ADAPTER: &str = "reference";

/// Production target adapter name (requires user credentials when live).
pub const CODEX_ADAPTER: &str = "codex";

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
    },
    Canceled {
        usage: UsageTotals,
    },
}

/// Active adapter child process.
pub struct AdapterRun {
    child: tokio::process::Child,
    canceled: Arc<AtomicBool>,
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

/// Spawns Codex CLI when `MUSEHUB_AGENT_CODEX_BIN` is set (live path — not verified without credentials).
pub async fn spawn_codex(task: &AgentTask, work_dir: &Path) -> Result<AdapterRun, AdapterError> {
    let binary =
        std::env::var("MUSEHUB_AGENT_CODEX_BIN").map_err(|_| AdapterError::Unavailable {
            adapter: CODEX_ADAPTER.to_string(),
            reason: "set MUSEHUB_AGENT_CODEX_BIN to the codex executable".to_string(),
        })?;
    spawn_ndjson_adapter(
        Path::new(&binary),
        &["exec", "--json"],
        task,
        work_dir,
        CODEX_ADAPTER,
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
    if !binary.is_file() {
        return Err(AdapterError::Unavailable {
            adapter: adapter_name.to_string(),
            reason: format!("missing executable {}", binary.display()),
        });
    }
    let payload = serde_json::to_string(task).map_err(|error| AdapterError::Failed {
        detail: error.to_string(),
    })?;
    let mut command = TokioCommand::new(binary);
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
            .write_all(payload.as_bytes())
            .await
            .map_err(|source| AdapterError::Io {
                step: "write adapter task",
                source,
            })?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|source| AdapterError::Io {
                step: "write adapter task",
                source,
            })?;
    }
    Ok(AdapterRun {
        child,
        canceled: Arc::new(AtomicBool::new(false)),
    })
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
    let mut stdout = run.child.stdout.take().ok_or(AdapterError::Failed {
        detail: "adapter stdout missing".to_string(),
    })?;
    let mut buffer = Vec::new();
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
            });
        }
        let mut line = String::new();
        match timeout(Duration::from_millis(250), reader.read_line(&mut line)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(_)) => {
                buffer.extend_from_slice(line.as_bytes());
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("usage") {
                        let usage = parse_usage_line(&event)?;
                        if budget_exceeded(&usage, budget_tokens, budget_microusd) {
                            run.request_cancel();
                            return Ok(RunOutcome::Failed {
                                code: "agent_budget_exceeded".to_string(),
                                detail: "usage exceeded configured limit".to_string(),
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
    drain_remaining_stdout(&mut reader, &mut buffer).await?;
    if run.canceled.load(Ordering::SeqCst) {
        return Ok(RunOutcome::Canceled {
            usage: parse_stream_usage(&buffer)?,
        });
    }
    if !status.success() {
        return Ok(RunOutcome::Failed {
            code: "agent_adapter_failed".to_string(),
            detail: format!("exit {}", status),
        });
    }
    let summary = parse_stream_summary(&buffer)?;
    Ok(RunOutcome::Succeeded {
        commit: summary.commit,
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
        if usage.tokens > limit {
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
}

fn parse_stream_summary(buffer: &[u8]) -> Result<StreamSummary, AdapterError> {
    let text = String::from_utf8_lossy(buffer);
    let mut commit = None;
    let mut usage = UsageTotals::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|error| AdapterError::Failed {
                detail: error.to_string(),
            })?;
        if value.get("type").and_then(|v| v.as_str()) == Some("completed") {
            commit = value
                .get("commit")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
        }
        if value.get("type").and_then(|v| v.as_str()) == Some("usage") {
            usage = parse_usage_line(&value)?;
        }
    }
    Ok(StreamSummary { commit, usage })
}

fn parse_stream_usage(buffer: &[u8]) -> Result<UsageTotals, AdapterError> {
    Ok(parse_stream_summary(buffer)?.usage)
}

fn parse_usage_line(value: &serde_json::Value) -> Result<UsageTotals, AdapterError> {
    let tokens = value.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let microusd = value.get("microusd").and_then(|v| v.as_u64()).unwrap_or(0);
    let budget_unknown = value
        .get("budget_unknown")
        .and_then(|v| v.as_bool())
        .unwrap_or(tokens == 0 && microusd == 0);
    Ok(UsageTotals {
        tokens,
        microusd,
        budget_unknown,
    })
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
    fn parse_completed_commit() {
        let input = r#"{"type":"started"}
{"type":"completed","kind":"implementation","commit":"abc"}
"#;
        let summary = parse_stream_summary(input.as_bytes()).expect("summary");
        assert_eq!(summary.commit.as_deref(), Some("abc"));
    }
}
