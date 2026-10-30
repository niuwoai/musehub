//! Rootless Docker probe executed through Colima SSH inside the Linux VM.
//!
//! The probe never talks to the macOS Docker socket and never runs as `root`.
//! It drives the rootless Engine's own socket
//! (`unix:///run/user/502/docker.sock`) through `/opt/homebrew/bin/colima ssh`
//! and `std::process::Command` argument arrays. No shell is involved on the host,
//! images are never pulled, and no container receives a volume or a Docker socket.
//!
//! # Scope
//!
//! Every report carries [`SCOPE`] (`linux_rootless_colima`). It proves, for the
//! measured Engine and container, that:
//!
//! * the Engine reports the `name=rootless` security option;
//! * [`EngineInfo::docker_root_dir`](RootlessReport::docker_root_dir) lives under
//!   `/home/` and is not the rootful `/var/lib/docker`;
//! * `docker build -` from stdin succeeds with BuildKit;
//! * a `--network none` container without mounted volumes cannot see a Docker
//!   socket.
//!
//! This is a single-organization feasibility probe. Rootless containers are not a
//! production multi-tenant isolation boundary.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use musehub_core::ErrorCode;

/// Stable scope label included in every report.
///
/// The Engine runs rootless inside the Colima Linux VM. This label must not be
/// read as evidence of production multi-tenant isolation.
pub const SCOPE: &str = "linux_rootless_colima";

/// Absolute path of the Colima CLI used to reach the Linux VM.
pub const COLIMA_BIN: &str = "/opt/homebrew/bin/colima";

/// Rootless Docker socket owned by uid 502 inside the Linux VM.
pub const ROOTLESS_DOCKER_HOST: &str = "unix:///run/user/502/docker.sock";

/// Base image already present in the rootless Engine. It is never pulled.
pub const PROBE_IMAGE: &str = "busybox:1.36.1";

/// Local tag of the probe image built from stdin.
pub const PROBE_TAG: &str = "musehub-rootless-probe:local";

/// Expected output of the built probe image.
pub const PROBE_OUTPUT: &str = "musehub-rootless";

/// Expected output of the container that checks for a Docker socket.
pub const NO_SOCKET_OUTPUT: &str = "no-socket";

/// Path a container would see if the Docker socket were mounted.
pub const SOCKET_PATH: &str = "/var/run/docker.sock";

/// `name=rootless` entry of `docker info` `.SecurityOptions`.
pub const ROOTLESS_SECURITY_OPTION: &str = "name=rootless";

/// Dockerfile sent to `docker build -` on standard input.
pub const DOCKERFILE: &str = "FROM busybox:1.36.1\nRUN printf musehub-rootless > /probe.txt\nCMD [\"cat\", \"/probe.txt\"]\n";

/// Script run inside the base image to prove no Docker socket is visible.
///
/// Exits with `17` when a socket exists so the failure cannot be confused with a
/// successful probe, and prints [`NO_SOCKET_OUTPUT`] otherwise.
pub const SOCKET_CHECK_SCRIPT: &str =
    "if [ -S /var/run/docker.sock ]; then exit 17; fi; printf no-socket";

/// Exit code used by [`SOCKET_CHECK_SCRIPT`] when the socket is visible.
pub const SOCKET_VISIBLE_EXIT_CODE: i32 = 17;

/// Docker root directory that only a rootful Engine uses.
pub const ROOTFUL_DOCKER_ROOT: &str = "/var/lib/docker";

/// Prefix every rootless per-user data directory is expected to start with.
pub const ROOTLESS_HOME_PREFIX: &str = "/home/";

/// Inputs of [`probe`].
///
/// The defaults point at the real rootless Engine inside the Colima Linux VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeConfig {
    /// Absolute path of the Colima CLI.
    pub colima_bin: PathBuf,
    /// Rootless Docker socket inside the Linux VM.
    pub docker_host: String,
    /// Base image used for the socket check; must already exist locally.
    pub image: String,
    /// Local tag the probe image is built as.
    pub build_tag: String,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            colima_bin: PathBuf::from(COLIMA_BIN),
            docker_host: ROOTLESS_DOCKER_HOST.to_string(),
            image: PROBE_IMAGE.to_string(),
            build_tag: PROBE_TAG.to_string(),
        }
    }
}

impl ProbeConfig {
    /// The default configuration: the rootless Engine of the Colima Linux VM.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Evidence collected by [`probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootlessReport {
    /// Always [`SCOPE`].
    pub scope: &'static str,
    /// `.DockerRootDir` reported by the rootless Engine.
    pub docker_root_dir: String,
    /// Always `true`: the Engine reported [`ROOTLESS_SECURITY_OPTION`].
    pub rootless: bool,
    /// Always `false`: the probe fails when a container can see [`SOCKET_PATH`].
    pub socket_visible_in_container: bool,
    /// Exit code of `docker build -`.
    pub build_exit_code: Option<i32>,
}

/// Exit status and captured streams of one `colima ssh ... docker ...` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    /// Exit code, or `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    /// Trimmed standard output.
    pub stdout: String,
    /// Trimmed standard error.
    pub stderr: String,
}

impl CommandOutcome {
    /// Whether the command exited with status `0`.
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Runs the full rootless probe against the configured Engine.
///
/// Steps, in order:
///
/// 1. `docker info --format {{json .}}` and validation of `.SecurityOptions` and
///    `.DockerRootDir`;
/// 2. `docker build -t PROBE_TAG -` with [`DOCKERFILE`] on standard input;
/// 3. `docker run --rm --network none PROBE_TAG cat /probe.txt`;
/// 4. `docker run --rm --network none --entrypoint /bin/sh PROBE_IMAGE -c
///    SOCKET_CHECK_SCRIPT`.
///
/// Every failure is a [`RootlessProbeError`] that names the failing command and
/// its stderr. A missing Colima binary, a stopped VM, or an unreachable rootless
/// daemon fails the probe; it is never skipped.
pub fn probe(config: &ProbeConfig) -> Result<RootlessReport, RootlessProbeError> {
    ensure_colima(config)?;

    let info_args = ["info", "--format", "{{json .}}"];
    let info_line = command_line(config, &info_args);
    let info = run(config, "docker info", &info_args, None)?;
    require_success("docker info", &info_line, &info)?;
    let engine = parse_engine_info(&info.stdout, &info_line)?;

    let build_args = ["build", "-t", &config.build_tag, "-"];
    let build_line = command_line(config, &build_args);
    let build = run(config, "docker build", &build_args, Some(DOCKERFILE))?;
    require_success("docker build", &build_line, &build)?;

    let run_args = [
        "run",
        "--rm",
        "--network",
        "none",
        &config.build_tag,
        "cat",
        "/probe.txt",
    ];
    let run_line = command_line(config, &run_args);
    let built = run(config, "docker run", &run_args, None)?;
    require_success("docker run", &run_line, &built)?;
    if built.stdout != PROBE_OUTPUT {
        return Err(RootlessProbeError::Failed {
            step: "docker run",
            command: run_line,
            stderr: format!(
                "expected the built image to print {PROBE_OUTPUT:?}, got {:?} (exit code {:?})",
                built.stdout, built.exit_code
            ),
        });
    }

    let socket_args = [
        "run",
        "--rm",
        "--network",
        "none",
        "--entrypoint",
        "/bin/sh",
        &config.image,
        "-c",
        SOCKET_CHECK_SCRIPT,
    ];
    let socket_line = command_line(config, &socket_args);
    let socket = run(config, "docker run socket check", &socket_args, None)?;
    if !socket.success() {
        let reason = if socket.exit_code == Some(SOCKET_VISIBLE_EXIT_CODE) {
            format!(
                "a container can see the Docker socket at {SOCKET_PATH}; refusing any isolation claim"
            )
        } else {
            format!(
                "socket check exited with {:?}; stdout {:?}; stderr {:?}",
                socket.exit_code, socket.stdout, socket.stderr
            )
        };
        return Err(RootlessProbeError::Failed {
            step: "docker run socket check",
            command: socket_line,
            stderr: reason,
        });
    }
    if socket.stdout != NO_SOCKET_OUTPUT {
        return Err(RootlessProbeError::Failed {
            step: "docker run socket check",
            command: socket_line,
            stderr: format!(
                "expected {NO_SOCKET_OUTPUT:?}, got {:?} (exit code {:?})",
                socket.stdout, socket.exit_code
            ),
        });
    }

    Ok(RootlessReport {
        scope: SCOPE,
        docker_root_dir: engine.docker_root_dir,
        rootless: engine.rootless,
        socket_visible_in_container: false,
        build_exit_code: build.exit_code,
    })
}

/// The exact argument array used for every Docker call.
///
/// The rendering is only used in error messages; the real invocation keeps each
/// argument separate and never passes through a shell.
pub fn command_line(config: &ProbeConfig, args: &[&str]) -> String {
    let mut parts = vec![
        config.colima_bin.display().to_string(),
        "ssh".to_string(),
        "--".to_string(),
        "env".to_string(),
        format!("DOCKER_HOST={}", config.docker_host),
        "docker".to_string(),
    ];
    parts.extend(args.iter().map(|argument| (*argument).to_string()));
    parts.join(" ")
}

fn ensure_colima(config: &ProbeConfig) -> Result<(), RootlessProbeError> {
    if config.colima_bin.is_file() {
        return Ok(());
    }
    Err(RootlessProbeError::Failed {
        step: "resolve colima binary",
        command: config.colima_bin.display().to_string(),
        stderr: format!(
            "{} does not exist; the probe uses absolute paths only and never a shell PATH lookup",
            config.colima_bin.display()
        ),
    })
}

fn docker_command(config: &ProbeConfig, args: &[&str]) -> Command {
    let mut command = Command::new(&config.colima_bin);
    command
        .arg("ssh")
        .arg("--")
        .arg("env")
        .arg(format!("DOCKER_HOST={}", config.docker_host))
        .arg("docker")
        .args(args);
    command
}

fn run(
    config: &ProbeConfig,
    step: &'static str,
    args: &[&str],
    stdin: Option<&str>,
) -> Result<CommandOutcome, RootlessProbeError> {
    let line = command_line(config, args);
    let mut command = docker_command(config, args);
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|source| RootlessProbeError::io(step, line.clone(), source))?;
    if let Some(input) = stdin {
        let mut handle = child
            .stdin
            .take()
            .ok_or_else(|| RootlessProbeError::Failed {
                step,
                command: line.clone(),
                stderr: "standard input pipe is not available".to_string(),
            })?;
        handle
            .write_all(input.as_bytes())
            .map_err(|source| RootlessProbeError::io(step, line.clone(), source))?;
        // Dropping the handle closes the pipe so `docker build -` sees EOF.
    }
    let output = child
        .wait_with_output()
        .map_err(|source| RootlessProbeError::io(step, line.clone(), source))?;
    Ok(outcome(&output))
}

fn outcome(output: &Output) -> CommandOutcome {
    CommandOutcome {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

fn require_success(
    step: &'static str,
    command: &str,
    outcome: &CommandOutcome,
) -> Result<(), RootlessProbeError> {
    if outcome.success() {
        return Ok(());
    }
    Err(RootlessProbeError::Failed {
        step,
        command: command.to_string(),
        stderr: format!(
            "exit code {:?}; stderr: {}; stdout: {}",
            outcome.exit_code, outcome.stderr, outcome.stdout
        ),
    })
}

/// Validated subset of `docker info --format {{json .}}`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EngineInfo {
    docker_root_dir: String,
    rootless: bool,
}

/// Validates that the Engine is rootless and stores data in a user home directory.
fn parse_engine_info(json: &str, command: &str) -> Result<EngineInfo, RootlessProbeError> {
    let reject = |reason: String| RootlessProbeError::Failed {
        step: "docker info",
        command: command.to_string(),
        stderr: reason,
    };
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| reject(format!("docker info did not return JSON ({error}): {json}")))?;

    let docker_root_dir = value
        .get("DockerRootDir")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| reject(format!("docker info has no string DockerRootDir: {json}")))?;
    if docker_root_dir == ROOTFUL_DOCKER_ROOT {
        return Err(reject(format!(
            "DockerRootDir is {ROOTFUL_DOCKER_ROOT}, which is the rootful Engine"
        )));
    }
    if !docker_root_dir.starts_with(ROOTLESS_HOME_PREFIX) {
        return Err(reject(format!(
            "DockerRootDir {docker_root_dir:?} does not start with {ROOTLESS_HOME_PREFIX:?}"
        )));
    }

    let security_options = value
        .get("SecurityOptions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| reject(format!("docker info has no SecurityOptions array: {json}")))?;
    let rootless = security_options.iter().any(|option| {
        option.as_str().is_some_and(|option| {
            option
                .split(',')
                .any(|part| part.trim() == ROOTLESS_SECURITY_OPTION)
        })
    });
    if !rootless {
        return Err(reject(format!(
            "SecurityOptions {security_options:?} do not contain {ROOTLESS_SECURITY_OPTION}"
        )));
    }

    Ok(EngineInfo {
        docker_root_dir: docker_root_dir.to_string(),
        rootless,
    })
}

/// Rootless probe failures. Every variant uses `rootless_probe_failed` and keeps
/// the failing command together with its stderr.
#[derive(Debug, thiserror::Error)]
pub enum RootlessProbeError {
    #[error("rootless probe step `{step}` failed: `{command}`: {stderr}")]
    Failed {
        step: &'static str,
        command: String,
        stderr: String,
    },
    #[error("rootless probe step `{step}` could not run `{command}`: {source}")]
    Io {
        step: &'static str,
        command: String,
        #[source]
        source: io::Error,
    },
}

impl RootlessProbeError {
    fn io(step: &'static str, command: String, source: io::Error) -> Self {
        Self::Io {
            step,
            command,
            source,
        }
    }
}

impl ErrorCode for RootlessProbeError {
    fn code(&self) -> &'static str {
        "rootless_probe_failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO_JSON: &str = r#"{
        "ID": "6cb51112",
        "DockerRootDir": "/home/rocky.guest/.local/share/docker",
        "SecurityOptions": ["name=seccomp,profile=builtin", "name=rootless", "name=cgroupns"]
    }"#;

    #[test]
    fn default_config_targets_the_rootless_socket() {
        let config = ProbeConfig::default();
        assert_eq!(config.colima_bin, PathBuf::from(COLIMA_BIN));
        assert_eq!(config.docker_host, ROOTLESS_DOCKER_HOST);
        assert_eq!(config.image, PROBE_IMAGE);
        assert_eq!(config.build_tag, PROBE_TAG);
        assert!(
            !config.docker_host.contains("/Users/"),
            "host socket leaked"
        );
    }

    #[test]
    fn dockerfile_builds_the_expected_image_from_stdin() {
        assert!(
            DOCKERFILE.starts_with("FROM busybox:1.36.1\n"),
            "{DOCKERFILE}"
        );
        assert!(
            DOCKERFILE.contains("RUN printf musehub-rootless > /probe.txt"),
            "{DOCKERFILE}"
        );
        assert!(
            DOCKERFILE
                .trim_end()
                .ends_with("CMD [\"cat\", \"/probe.txt\"]"),
            "{DOCKERFILE}"
        );
    }

    #[test]
    fn command_line_keeps_each_argument_separate() {
        let config = ProbeConfig::default();
        let line = command_line(&config, &["info", "--format", "{{json .}}"]);
        assert_eq!(
            line,
            "/opt/homebrew/bin/colima ssh -- env DOCKER_HOST=unix:///run/user/502/docker.sock docker info --format {{json .}}"
        );
        let command = docker_command(&config, &["info"]);
        assert_eq!(command.get_program(), config.colima_bin.as_os_str());
        let args: Vec<String> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "ssh",
                "--",
                "env",
                "DOCKER_HOST=unix:///run/user/502/docker.sock",
                "docker",
                "info"
            ]
        );
    }

    #[test]
    fn rootless_engine_info_is_accepted() {
        let info = parse_engine_info(INFO_JSON, "docker info").expect("rootless info");
        assert_eq!(
            info.docker_root_dir,
            "/home/rocky.guest/.local/share/docker"
        );
        assert!(info.rootless);
    }

    #[test]
    fn rootful_or_homeless_docker_root_is_rejected() {
        for json in [
            r#"{"DockerRootDir": "/var/lib/docker", "SecurityOptions": ["name=rootless"]}"#,
            r#"{"DockerRootDir": "/root/.local/share/docker", "SecurityOptions": ["name=rootless"]}"#,
            r#"{"DockerRootDir": "/home/u/.local/share/docker", "SecurityOptions": ["name=seccomp"]}"#,
            r#"{"SecurityOptions": ["name=rootless"]}"#,
            "not json",
        ] {
            let error = parse_engine_info(json, "docker info").expect_err("must be rejected");
            assert_eq!(error.code(), "rootless_probe_failed");
        }
    }

    #[test]
    fn failures_name_the_command_and_stderr() {
        let error = RootlessProbeError::Failed {
            step: "docker build",
            command: "colima ssh -- docker build -t x -".to_string(),
            stderr: "no such image".to_string(),
        };
        assert_eq!(error.code(), "rootless_probe_failed");
        let message = error.to_string();
        assert!(message.contains("docker build"), "{message}");
        assert!(message.contains("colima ssh -- docker build"), "{message}");
        assert!(message.contains("no such image"), "{message}");
    }

    #[test]
    fn missing_colima_binary_fails_without_starting_a_process() {
        let config = ProbeConfig {
            colima_bin: PathBuf::from("/nonexistent/colima"),
            ..ProbeConfig::default()
        };
        let error = probe(&config).expect_err("missing binary must fail");
        assert_eq!(error.code(), "rootless_probe_failed");
        let message = error.to_string();
        assert!(message.contains("/nonexistent/colima"), "{message}");
        assert!(message.contains("does not exist"), "{message}");
    }
}
