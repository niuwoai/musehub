//! Local process probe for workspace confinement, timeout, and cancellation.
//!
//! This does not verify Linux rootless containers, cgroups, network policy,
//! or OCI image isolation. Those checks require a Linux host.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use musehub_core::ErrorCode;

/// Stable scope label included in every report.
///
/// The probe only controls a local process. It is not evidence of rootless isolation.
pub const SCOPE: &str = "local_process_only";

const MAX_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Evidence from one local process invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessReport {
    /// Always [`SCOPE`], so callers cannot confuse this with container isolation.
    pub scope: &'static str,
    pub workspace: PathBuf,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub canceled: bool,
}

/// A running process that can be canceled by the caller.
#[derive(Debug)]
pub struct RunningProcess {
    child: Child,
    workspace: PathBuf,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
}

impl RunningProcess {
    /// Stops the process and waits until it has exited.
    pub fn cancel(mut self) -> Result<ProcessReport, RunnerProbeError> {
        self.child
            .kill()
            .map_err(|source| RunnerProbeError::io("cancel process", source))?;
        let status = self
            .child
            .wait()
            .map_err(|source| RunnerProbeError::io("wait for canceled process", source))?;
        Ok(ProcessReport {
            scope: SCOPE,
            workspace: self.workspace,
            exit_code: status.code(),
            stdout: read_pipe(self.stdout.as_mut())?,
            stderr: read_pipe(self.stderr.as_mut())?,
            canceled: true,
        })
    }

    /// The workspace assigned to the process.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}

/// Runs `program args` inside `workspace` with only `environment` exported.
///
/// The parent environment is cleared. Relative paths in the command are resolved
/// inside the workspace, and `..` is rejected before the process starts.
pub fn run_confined(
    workspace: &Path,
    program: &Path,
    args: &[&str],
    environment: &HashMap<String, String>,
    timeout: Duration,
) -> Result<ProcessReport, RunnerProbeError> {
    let mut running = spawn_confined(workspace, program, args, environment)?;
    wait_with_timeout(&mut running, timeout)
}

/// Starts a confined process and returns its cancellation handle.
pub fn spawn_confined(
    workspace: &Path,
    program: &Path,
    args: &[&str],
    environment: &HashMap<String, String>,
) -> Result<RunningProcess, RunnerProbeError> {
    validate_workspace(workspace)?;
    validate_program(program)?;
    for arg in args {
        reject_workspace_escape(workspace, arg)?;
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(workspace)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|source| RunnerProbeError::io("spawn confined process", source))?;
    Ok(RunningProcess {
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
        child,
        workspace: workspace.to_path_buf(),
    })
}

fn wait_with_timeout(
    running: &mut RunningProcess,
    timeout: Duration,
) -> Result<ProcessReport, RunnerProbeError> {
    if timeout.is_zero() || timeout > MAX_TIMEOUT {
        let _ = running.child.kill();
        let _ = running.child.wait();
        return Err(RunnerProbeError::Failed {
            step: "validate timeout",
            stderr: format!(
                "timeout must be between 1ms and {}ms",
                MAX_TIMEOUT.as_millis()
            ),
        });
    }

    let started = Instant::now();
    loop {
        match running.child.try_wait() {
            Ok(Some(status)) => {
                return Ok(ProcessReport {
                    scope: SCOPE,
                    workspace: running.workspace.clone(),
                    exit_code: status.code(),
                    stdout: read_pipe(running.stdout.as_mut())?,
                    stderr: read_pipe(running.stderr.as_mut())?,
                    canceled: false,
                });
            }
            Ok(None) if started.elapsed() >= timeout => {
                running
                    .child
                    .kill()
                    .map_err(|source| RunnerProbeError::io("kill timed out process", source))?;
                let status = running
                    .child
                    .wait()
                    .map_err(|source| RunnerProbeError::io("wait for timed out process", source))?;
                let stderr = read_pipe(running.stderr.as_mut())?;
                return Err(RunnerProbeError::Failed {
                    step: "process timeout",
                    stderr: format!(
                        "process exceeded {}ms and was terminated (status {status}); stderr: {stderr}",
                        timeout.as_millis()
                    ),
                });
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(source) => return Err(RunnerProbeError::io("wait for confined process", source)),
        }
    }
}

fn validate_workspace(workspace: &Path) -> Result<(), RunnerProbeError> {
    let metadata = fs::metadata(workspace)
        .map_err(|source| RunnerProbeError::io("inspect workspace", source))?;
    if !metadata.is_dir() {
        return Err(RunnerProbeError::Failed {
            step: "inspect workspace",
            stderr: format!("workspace {} is not a directory", workspace.display()),
        });
    }
    Ok(())
}

fn validate_program(program: &Path) -> Result<(), RunnerProbeError> {
    if !program.is_absolute() {
        return Err(RunnerProbeError::Failed {
            step: "validate program",
            stderr: format!("program {} must be an absolute path", program.display()),
        });
    }
    Ok(())
}

fn reject_workspace_escape(workspace: &Path, value: &str) -> Result<(), RunnerProbeError> {
    let workspace = fs::canonicalize(workspace)
        .map_err(|source| RunnerProbeError::io("canonicalize workspace", source))?;
    for candidate in shell_path_candidates(value) {
        let path = Path::new(candidate);
        if !path.is_relative() {
            continue;
        }
        let resolved = normalize_lexical(&workspace.join(path));
        if !resolved.starts_with(&workspace) {
            return Err(RunnerProbeError::Failed {
                step: "reject workspace escape",
                stderr: format!(
                    "path {candidate:?} resolves to {} outside workspace {}",
                    resolved.display(),
                    workspace.display()
                ),
            });
        }
    }
    Ok(())
}

fn shell_path_candidates(value: &str) -> Vec<&str> {
    value
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    ';' | '&' | '|' | '<' | '>' | '(' | ')' | '`' | '"' | '\''
                )
        })
        .filter(|part| !part.is_empty())
        .collect()
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn read_pipe(pipe: Option<&mut impl Read>) -> Result<String, RunnerProbeError> {
    let mut buffer = String::new();
    if let Some(pipe) = pipe {
        pipe.read_to_string(&mut buffer)
            .map_err(|source| RunnerProbeError::io("read process output", source))?;
    }
    Ok(buffer)
}

/// Local process probe failures. Every variant uses `runner_probe_failed`.
#[derive(Debug, thiserror::Error)]
pub enum RunnerProbeError {
    #[error("runner probe step `{step}` failed: {stderr}")]
    Failed { step: &'static str, stderr: String },
    #[error("runner probe step `{step}` could not run: {source}")]
    Io {
        step: &'static str,
        #[source]
        source: io::Error,
    },
}

impl RunnerProbeError {
    fn io(step: &'static str, source: io::Error) -> Self {
        Self::Io { step, source }
    }
}

impl ErrorCode for RunnerProbeError {
    fn code(&self) -> &'static str {
        "runner_probe_failed"
    }
}
