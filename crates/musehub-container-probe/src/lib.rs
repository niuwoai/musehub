//! Container probe for digest-pinned task execution without a Docker socket.
//!
//! The probe drives the local Docker Engine through absolute binary paths and
//! `std::process::Command` argument arrays. It never uses a shell, never pulls an
//! image, never mounts the Docker socket, and never runs a privileged container.
//!
//! # Scope
//!
//! Every report carries [`SCOPE`] (`colima_docker_vm`): the Engine lives inside a
//! Colima virtual machine, so the container boundary is the VM boundary. This is
//! **not** rootless Docker and **not** a production isolation boundary. The probe
//! only demonstrates three facts for the measured container:
//!
//! * the image reference is an immutable `name@sha256:...` digest;
//! * the container runs with `--network none` and no Docker socket mount;
//! * the image ID of the container that actually ran equals the image ID of the
//!   digest that was requested.
//!
//! # Why two probes
//!
//! [`run_digest`] runs `docker run --rm`, which deletes the container when the
//! command exits, so its image ID can no longer be inspected afterwards.
//! [`compose_digest_probe`] therefore creates a named container with
//! `docker-compose`, inspects `.Image` while the container still exists, compares
//! it with `docker image inspect --format {{.Id}}`, and only then removes the
//! container with `docker-compose down --remove-orphans`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use musehub_core::ErrorCode;

/// Stable scope label included in every report.
///
/// The Docker Engine runs inside a Colima virtual machine. This label must not be
/// read as evidence of rootless Docker or of production isolation.
pub const SCOPE: &str = "colima_docker_vm";

/// Absolute path of the Docker CLI used by the probe.
pub const DOCKER_BIN: &str = "/opt/homebrew/bin/docker";

/// Absolute path of the standalone `docker-compose` v2 CLI used by the probe.
///
/// The `docker compose` plugin subcommand does not exist on this host, so the
/// standalone executable is always invoked with fixed arguments.
pub const DOCKER_COMPOSE_BIN: &str = "/opt/homebrew/bin/docker-compose";

/// File name of the generated Compose manifest inside the caller's project directory.
pub const COMPOSE_FILE_NAME: &str = "compose.yaml";

/// Compose service name of the probe container.
pub const COMPOSE_SERVICE: &str = "probe";

/// Text printed by the probe container.
pub const PROBE_OUTPUT: &str = "musehub-probe";

/// Marker that must never appear in a probe argument or in a container mount.
const SOCKET_MARKER: &str = "docker.sock";

/// Container-isolation flags the probe refuses to forward.
const FORBIDDEN_FLAGS: [&str; 14] = [
    "-v",
    "--volume",
    "--mount",
    "--privileged",
    "--pid",
    "--network",
    "--net",
    "--device",
    "--cap-add",
    "--ipc",
    "--uts",
    "--userns",
    "-p",
    "--publish",
];

/// Evidence from one `docker run --rm --network none` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// Always [`SCOPE`].
    pub scope: &'static str,
    /// The exact `name@sha256:...` reference that was started.
    pub image_digest: String,
    /// Always `None`: `--rm` deletes the container before it can be inspected.
    ///
    /// Use [`compose_digest_probe`] when a verified running image ID is needed.
    pub running_image_id: Option<String>,
    /// Container exit code, or `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    /// Always `false`: socket mounts are rejected before Docker is started.
    pub socket_mounted: bool,
    /// Trimmed standard output of the Docker CLI.
    pub stdout: String,
    /// Trimmed standard error of the Docker CLI.
    pub stderr: String,
}

/// Evidence from one named Compose container run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeReport {
    /// Always [`SCOPE`].
    pub scope: &'static str,
    /// Compose project name requested by the caller.
    pub project_name: String,
    /// The exact digest written into the Compose manifest.
    pub image_digest: String,
    /// `.Image` of the container that actually ran, read with `docker inspect`.
    pub running_image_id: String,
    /// `.Id` of the requested digest, read with `docker image inspect`.
    pub expected_image_id: String,
    /// Probe container exit code.
    pub exit_code: Option<i32>,
    /// Always `false`: the mounts of the running container were inspected.
    pub socket_mounted: bool,
    /// Trimmed standard output of `docker-compose up`.
    pub stdout: String,
    /// Trimmed standard error of `docker-compose up`.
    pub stderr: String,
}

impl ComposeReport {
    /// Standard output and standard error joined for human inspection.
    pub fn combined_output(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// Exit status and captured streams of one external command.
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

/// Runs `docker run --rm --network none IMAGE ARGS` without any mount capability.
///
/// Arguments containing `docker.sock` and every isolation flag listed in
/// `FORBIDDEN_FLAGS` are rejected before Docker is started. The image reference
/// must be a `name@sha256:` digest so the run cannot drift to another image.
pub fn run_digest(
    image_digest: &str,
    command_args: &[&str],
) -> Result<RunReport, ContainerProbeError> {
    let docker = docker_binary()?;
    run_digest_with_docker(&docker, image_digest, command_args)
}

/// [`run_digest`] with an explicit Docker CLI path.
///
/// Validation of the digest and of the arguments happens before the binary is
/// touched, so a caller can pass a path to a sentinel program and prove that a
/// rejected argument set never executes Docker.
pub fn run_digest_with_docker(
    docker: &Path,
    image_digest: &str,
    command_args: &[&str],
) -> Result<RunReport, ContainerProbeError> {
    validate_image_digest(image_digest)?;
    validate_command_args("docker run", command_args)?;
    ensure_daemon(docker)?;

    let mut command = Command::new(docker);
    command
        .arg("run")
        .arg("--rm")
        .arg("--network")
        .arg("none")
        .arg(image_digest)
        .args(command_args);
    let output = run("docker run", &mut command)?;
    let outcome = outcome(&output);
    if !outcome.success() {
        return Err(ContainerProbeError::Failed {
            step: "docker run",
            stderr: format!(
                "exit code {:?}: {}{}",
                outcome.exit_code, outcome.stderr, outcome.stdout
            ),
        });
    }
    Ok(RunReport {
        scope: SCOPE,
        image_digest: image_digest.to_string(),
        running_image_id: None,
        exit_code: outcome.exit_code,
        socket_mounted: false,
        stdout: outcome.stdout,
        stderr: outcome.stderr,
    })
}

/// Starts the digest-pinned probe container, verifies its image ID, then cleans up.
///
/// The generated `compose.yaml` in `project_dir` pins `image` to `image_digest`,
/// sets `network_mode: none`, and never declares volumes, `privileged`, or `pid`.
/// `project_name` must be lowercase ASCII letters, digits, and hyphens.
///
/// The container is removed with `docker-compose down --remove-orphans` whether the
/// probe succeeds or fails.
pub fn compose_digest_probe(
    project_dir: &Path,
    project_name: &str,
    image_digest: &str,
) -> Result<ComposeReport, ContainerProbeError> {
    validate_project_name(project_name)?;
    validate_image_digest(image_digest)?;
    let docker = docker_binary()?;
    let compose = compose_binary()?;
    ensure_daemon(&docker)?;
    write_compose_file(project_dir, image_digest)?;

    let result = inspect_compose_probe(&docker, &compose, project_dir, project_name, image_digest);
    let cleanup = compose_down_with(&compose, project_dir, project_name);
    match (result, cleanup) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(ContainerProbeError::Failed {
            step: "docker-compose down",
            stderr: format!("{error}; cleanup also failed: {cleanup_error}"),
        }),
        (Ok(_), Err(error)) => Err(error),
    }
}

/// Writes the probe Compose manifest into `project_dir` and returns its path.
pub fn write_compose_file(
    project_dir: &Path,
    image_digest: &str,
) -> Result<PathBuf, ContainerProbeError> {
    validate_image_digest(image_digest)?;
    let metadata = fs::metadata(project_dir)
        .map_err(|source| ContainerProbeError::io("inspect project directory", source))?;
    if !metadata.is_dir() {
        return Err(ContainerProbeError::Failed {
            step: "inspect project directory",
            stderr: format!("{} is not a directory", project_dir.display()),
        });
    }
    let path = project_dir.join(COMPOSE_FILE_NAME);
    fs::write(&path, compose_manifest(image_digest))
        .map_err(|source| ContainerProbeError::io("write compose file", source))?;
    Ok(path)
}

/// Runs `docker-compose up --abort-on-container-exit --exit-code-from probe`.
pub fn compose_up(
    project_dir: &Path,
    project_name: &str,
) -> Result<CommandOutcome, ContainerProbeError> {
    validate_project_name(project_name)?;
    let docker = docker_binary()?;
    ensure_daemon(&docker)?;
    let compose = compose_binary()?;
    compose_up_with(&compose, project_dir, project_name)
}

/// Finds the container ID of the probe service with `docker-compose ps -aq`.
pub fn compose_container_id(
    project_dir: &Path,
    project_name: &str,
) -> Result<String, ContainerProbeError> {
    validate_project_name(project_name)?;
    let compose = compose_binary()?;
    compose_container_id_with(&compose, project_dir, project_name)
}

/// Removes the probe project with `docker-compose down --remove-orphans`.
pub fn compose_down(project_dir: &Path, project_name: &str) -> Result<(), ContainerProbeError> {
    validate_project_name(project_name)?;
    let compose = compose_binary()?;
    compose_down_with(&compose, project_dir, project_name)
}

/// Reads `.Image` of an existing container with `docker inspect`.
pub fn container_image_id(container_id: &str) -> Result<String, ContainerProbeError> {
    let docker = docker_binary()?;
    inspect_container_image(&docker, container_id)
}

/// Reads `.Id` of a digest-pinned image with `docker image inspect`.
pub fn image_id_for_digest(image_digest: &str) -> Result<String, ContainerProbeError> {
    validate_image_digest(image_digest)?;
    let docker = docker_binary()?;
    inspect_image_id(&docker, image_digest)
}

/// Lists every mount source attached to an existing container.
pub fn container_mount_sources(container_id: &str) -> Result<Vec<String>, ContainerProbeError> {
    let docker = docker_binary()?;
    inspect_mount_sources(&docker, container_id)
}

/// The exact manifest written by [`write_compose_file`].
pub fn compose_manifest(image_digest: &str) -> String {
    format!(
        "services:\n  {COMPOSE_SERVICE}:\n    image: {image_digest}\n    network_mode: none\n    pull_policy: never\n    command: [\"printf\", \"{PROBE_OUTPUT}\"]\n"
    )
}

fn inspect_compose_probe(
    docker: &Path,
    compose: &Path,
    project_dir: &Path,
    project_name: &str,
    image_digest: &str,
) -> Result<ComposeReport, ContainerProbeError> {
    let up = compose_up_with(compose, project_dir, project_name)?;
    if !up.success() {
        return Err(ContainerProbeError::Failed {
            step: "docker-compose up",
            stderr: format!(
                "probe container exited with {:?}: {}{}",
                up.exit_code, up.stderr, up.stdout
            ),
        });
    }

    let container_id = compose_container_id_with(compose, project_dir, project_name)?;
    let running_image_id = inspect_container_image(docker, &container_id)?;
    let expected_image_id = inspect_image_id(docker, image_digest)?;
    let mounts = inspect_mount_sources(docker, &container_id)?;

    let socket_mounts: Vec<&String> = mounts
        .iter()
        .filter(|source| source.to_lowercase().contains(SOCKET_MARKER))
        .collect();
    if !socket_mounts.is_empty() {
        return Err(ContainerProbeError::Failed {
            step: "inspect container mounts",
            stderr: format!("container {container_id} mounts the Docker socket: {socket_mounts:?}"),
        });
    }
    if running_image_id != expected_image_id {
        return Err(ContainerProbeError::Failed {
            step: "compare image ids",
            stderr: format!(
                "container {container_id} ran image {running_image_id}, but digest {image_digest} resolves to {expected_image_id}"
            ),
        });
    }

    Ok(ComposeReport {
        scope: SCOPE,
        project_name: project_name.to_string(),
        image_digest: image_digest.to_string(),
        running_image_id,
        expected_image_id,
        exit_code: up.exit_code,
        socket_mounted: false,
        stdout: up.stdout,
        stderr: up.stderr,
    })
}

fn compose_up_with(
    compose: &Path,
    project_dir: &Path,
    project_name: &str,
) -> Result<CommandOutcome, ContainerProbeError> {
    let mut command = compose_command(compose, project_dir, project_name);
    command
        .arg("up")
        .arg("--abort-on-container-exit")
        .arg("--exit-code-from")
        .arg(COMPOSE_SERVICE);
    let output = run("docker-compose up", &mut command)?;
    Ok(outcome(&output))
}

fn compose_container_id_with(
    compose: &Path,
    project_dir: &Path,
    project_name: &str,
) -> Result<String, ContainerProbeError> {
    let mut command = compose_command(compose, project_dir, project_name);
    command.arg("ps").arg("--all").arg("--quiet");
    let output = run("docker-compose ps", &mut command)?;
    let outcome = outcome(&output);
    let container_id = outcome
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string);
    match container_id {
        Some(container_id) => Ok(container_id),
        None => Err(ContainerProbeError::Failed {
            step: "docker-compose ps",
            stderr: format!(
                "no container found for project {project_name}: {}",
                outcome.stderr
            ),
        }),
    }
}

fn compose_down_with(
    compose: &Path,
    project_dir: &Path,
    project_name: &str,
) -> Result<(), ContainerProbeError> {
    let mut command = compose_command(compose, project_dir, project_name);
    command.arg("down").arg("--remove-orphans");
    let output = run("docker-compose down", &mut command)?;
    let outcome = outcome(&output);
    if outcome.success() {
        Ok(())
    } else {
        Err(ContainerProbeError::Failed {
            step: "docker-compose down",
            stderr: format!(
                "exit code {:?}: {}{}",
                outcome.exit_code, outcome.stderr, outcome.stdout
            ),
        })
    }
}

fn compose_command(compose: &Path, project_dir: &Path, project_name: &str) -> Command {
    let mut command = Command::new(compose);
    command
        .arg("--project-name")
        .arg(project_name)
        .arg("--file")
        .arg(project_dir.join(COMPOSE_FILE_NAME))
        .current_dir(project_dir);
    command
}

fn inspect_container_image(
    docker: &Path,
    container_id: &str,
) -> Result<String, ContainerProbeError> {
    let output = docker_inspect(docker, "{{.Image}}", container_id)?;
    require_image_id("docker inspect {{.Image}}", output)
}

fn inspect_image_id(docker: &Path, image_digest: &str) -> Result<String, ContainerProbeError> {
    let mut command = Command::new(docker);
    command
        .arg("image")
        .arg("inspect")
        .arg("--format")
        .arg("{{.Id}}")
        .arg(image_digest);
    let output = run("docker image inspect", &mut command)?;
    let outcome = outcome(&output);
    if !outcome.success() {
        return Err(ContainerProbeError::Failed {
            step: "docker image inspect",
            stderr: format!(
                "exit code {:?}: {}{}",
                outcome.exit_code, outcome.stderr, outcome.stdout
            ),
        });
    }
    require_image_id("docker image inspect", outcome.stdout)
}

fn inspect_mount_sources(
    docker: &Path,
    container_id: &str,
) -> Result<Vec<String>, ContainerProbeError> {
    let output = docker_inspect(
        docker,
        "{{range .Mounts}}{{.Source}}{{\"\\n\"}}{{end}}",
        container_id,
    )?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn docker_inspect(
    docker: &Path,
    format: &str,
    container_id: &str,
) -> Result<String, ContainerProbeError> {
    let mut command = Command::new(docker);
    command
        .arg("inspect")
        .arg("--format")
        .arg(format)
        .arg(container_id);
    let output = run("docker inspect", &mut command)?;
    let outcome = outcome(&output);
    if !outcome.success() {
        return Err(ContainerProbeError::Failed {
            step: "docker inspect",
            stderr: format!(
                "exit code {:?}: {}{}",
                outcome.exit_code, outcome.stderr, outcome.stdout
            ),
        });
    }
    Ok(outcome.stdout)
}

fn require_image_id(step: &'static str, value: String) -> Result<String, ContainerProbeError> {
    if value.starts_with("sha256:") && value.len() > "sha256:".len() {
        Ok(value)
    } else {
        Err(ContainerProbeError::Failed {
            step,
            stderr: format!("expected an image ID starting with `sha256:`, got {value:?}"),
        })
    }
}

/// Verifies that the Docker CLI exists at its absolute path.
pub fn docker_binary() -> Result<PathBuf, ContainerProbeError> {
    resolve_binary("docker", "resolve docker binary", DOCKER_BIN)
}

/// Verifies that the standalone `docker-compose` CLI exists at its absolute path.
pub fn compose_binary() -> Result<PathBuf, ContainerProbeError> {
    resolve_binary(
        "docker-compose",
        "resolve docker-compose binary",
        DOCKER_COMPOSE_BIN,
    )
}

fn resolve_binary(
    name: &'static str,
    step: &'static str,
    candidate: &'static str,
) -> Result<PathBuf, ContainerProbeError> {
    let path = Path::new(candidate);
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    Err(ContainerProbeError::Failed {
        step,
        stderr: format!(
            "{name} executable {candidate} does not exist; the probe uses absolute paths only and never a shell PATH lookup"
        ),
    })
}

/// Fails with the `docker info` stderr when the Docker daemon is unreachable.
pub fn ensure_daemon(docker: &Path) -> Result<(), ContainerProbeError> {
    let mut command = Command::new(docker);
    command
        .arg("info")
        .arg("--format")
        .arg("{{.ServerVersion}}");
    let output = run("docker info", &mut command)?;
    let outcome = outcome(&output);
    if outcome.success() && !outcome.stdout.is_empty() {
        return Ok(());
    }
    Err(ContainerProbeError::Failed {
        step: "docker info",
        stderr: format!(
            "Docker daemon is not available (exit code {:?}): {}",
            outcome.exit_code, outcome.stderr
        ),
    })
}

/// Rejects tag-only references and any digest that is not `sha256:` hex.
fn validate_image_digest(image_digest: &str) -> Result<(), ContainerProbeError> {
    let rejection = |reason: &str| ContainerProbeError::Failed {
        step: "validate image digest",
        stderr: format!("{image_digest:?} is not a sha256 image digest: {reason}"),
    };
    let (name, digest) = image_digest
        .split_once('@')
        .ok_or_else(|| rejection("missing `@`; tag-only references are not immutable"))?;
    if name.is_empty() {
        return Err(rejection("missing image name"));
    }
    let hex = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| rejection("digest must start with `sha256:`"))?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) || !hex.is_ascii() {
        return Err(rejection("digest must be 64 hexadecimal characters"));
    }
    Ok(())
}

/// Rejects socket paths and isolation flags before any Docker process starts.
fn validate_command_args(
    step: &'static str,
    command_args: &[&str],
) -> Result<(), ContainerProbeError> {
    let socket_argument = command_args
        .iter()
        .find(|argument| argument.to_lowercase().contains(SOCKET_MARKER));
    if let Some(argument) = socket_argument {
        return Err(ContainerProbeError::ForbiddenArgument {
            step,
            argument: (*argument).to_string(),
        });
    }
    for argument in command_args {
        if let Some(flag) = forbidden_flag(argument) {
            return Err(ContainerProbeError::ForbiddenArgument {
                step,
                argument: flag.to_string(),
            });
        }
    }
    Ok(())
}

fn forbidden_flag(argument: &str) -> Option<&str> {
    let base = argument.split('=').next().unwrap_or(argument);
    if FORBIDDEN_FLAGS.contains(&base) {
        return Some(base);
    }
    if argument.starts_with("-v") && !argument.starts_with("--") {
        return Some("-v");
    }
    None
}

/// Container names must be lowercase ASCII letters, digits, and hyphens.
fn validate_project_name(project_name: &str) -> Result<(), ContainerProbeError> {
    let valid = !project_name.is_empty()
        && project_name.len() <= 63
        && project_name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && project_name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(ContainerProbeError::Failed {
            step: "validate project name",
            stderr: format!(
                "{project_name:?} must be 1-63 lowercase ASCII letters, digits, or hyphens and start with a letter or digit"
            ),
        })
    }
}

fn run(step: &'static str, command: &mut Command) -> Result<Output, ContainerProbeError> {
    command
        .stdin(Stdio::null())
        .output()
        .map_err(|source| ContainerProbeError::io(step, source))
}

fn outcome(output: &Output) -> CommandOutcome {
    CommandOutcome {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

/// Container probe failures. Every variant uses `container_probe_failed`.
#[derive(Debug, thiserror::Error)]
pub enum ContainerProbeError {
    #[error("container probe step `{step}` refused argument {argument:?}: Docker socket mounts and isolation-changing flags are not allowed")]
    ForbiddenArgument {
        step: &'static str,
        argument: String,
    },
    #[error("container probe step `{step}` failed: {stderr}")]
    Failed { step: &'static str, stderr: String },
    #[error("container probe step `{step}` could not run: {source}")]
    Io {
        step: &'static str,
        #[source]
        source: io::Error,
    },
}

impl ContainerProbeError {
    fn io(step: &'static str, source: io::Error) -> Self {
        Self::Io { step, source }
    }
}

impl ErrorCode for ContainerProbeError {
    fn code(&self) -> &'static str {
        "container_probe_failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str =
        "busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662";

    #[test]
    fn manifest_pins_digest_and_declares_no_mounts() {
        let manifest = compose_manifest(DIGEST);
        assert!(manifest.contains(&format!("image: {DIGEST}")), "{manifest}");
        assert!(manifest.contains("network_mode: none"), "{manifest}");
        assert!(manifest.contains("pull_policy: never"), "{manifest}");
        assert!(
            manifest.contains("command: [\"printf\", \"musehub-probe\"]"),
            "{manifest}"
        );
        assert!(!manifest.contains("volumes"), "{manifest}");
        assert!(!manifest.contains("privileged"), "{manifest}");
        assert!(!manifest.contains("docker.sock"), "{manifest}");
        assert!(!manifest.contains("pid:"), "{manifest}");
    }

    #[test]
    fn socket_and_isolation_arguments_are_rejected() {
        for args in [
            &["-v", "/var/run/docker.sock:/var/run/docker.sock"][..],
            &["--mount", "type=bind,source=/var/run/docker.sock"][..],
            &["--privileged"][..],
            &["--pid=host"][..],
            &["--network=host"][..],
            &["-v/tmp:/tmp"][..],
        ] {
            let error = validate_command_args("docker run", args).expect_err("must be rejected");
            assert_eq!(error.code(), "container_probe_failed");
        }
    }

    #[test]
    fn harmless_arguments_are_allowed() {
        validate_command_args("docker run", &["printf", "musehub-probe"]).expect("allowed");
        validate_command_args("docker run", &[]).expect("allowed");
    }

    #[test]
    fn digest_validation_requires_sha256_pin() {
        validate_image_digest(DIGEST).expect("valid digest");
        for invalid in [
            "busybox:1.36.1",
            "busybox@sha256:abc",
            "@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662",
            "busybox@md5:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662",
            "busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b1166g",
        ] {
            let error = validate_image_digest(invalid).expect_err("must be rejected");
            assert_eq!(error.code(), "container_probe_failed");
        }
    }

    #[test]
    fn project_name_rejects_uppercase_and_symbols() {
        validate_project_name("musehub-probe-1").expect("valid name");
        for invalid in [
            "",
            "MuseHub",
            "musehub_probe",
            "-probe",
            "probe ",
            "probe/1",
        ] {
            let error = validate_project_name(invalid).expect_err("must be rejected");
            assert_eq!(error.code(), "container_probe_failed");
        }
    }

    #[test]
    fn missing_binary_paths_are_named_in_the_error() {
        let error = resolve_binary("docker", "resolve docker binary", "/nonexistent/docker")
            .expect_err("must be rejected");
        assert!(error.to_string().contains("/nonexistent/docker"), "{error}");
    }
}
