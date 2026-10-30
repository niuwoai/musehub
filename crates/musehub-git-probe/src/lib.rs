//! Local Git probe: proves that standard `git` subprocesses can host a bare
//! repository, clone it, record a non-empty commit and push it back.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use musehub_core::ErrorCode;

/// Identity used for probe commits; never written to global git config.
pub const PROBE_EMAIL: &str = "probe@musehub.local";
/// Author/committer name used for probe commits.
pub const PROBE_USER: &str = "musehub-probe";
/// Branch the probe pushes to on the bare repository.
pub const PROBE_BRANCH: &str = "main";
/// File created in the clone so the probe commit has a non-empty tree.
pub const PROBE_FILE: &str = "probe.txt";
/// Commit message used by the probe.
pub const PROBE_MESSAGE: &str = "probe: non-empty commit";

/// Evidence collected by a successful probe run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// Directory holding the bare repository and the working clone.
    pub root: PathBuf,
    /// Bare repository created by `git init --bare`.
    pub bare_repo: PathBuf,
    /// Working clone that records the probe commit.
    pub work_tree: PathBuf,
    /// Commit object id reported by the bare repository as `HEAD`.
    pub commit: String,
    /// Tree object id of that commit.
    pub tree: String,
    /// `ls-tree -r` entries of that tree; must not be empty.
    pub tree_entries: Vec<String>,
    /// Branch name the bare repository's `HEAD` points at.
    pub remote_head: String,
}

impl ProbeReport {
    /// Removes the probe workspace from disk.
    pub fn cleanup(&self) -> std::io::Result<()> {
        fs::remove_dir_all(&self.root)
    }
}

/// Runs the probe inside a fresh directory under the system temp directory.
///
/// The workspace is kept on disk so failures can be inspected; call
/// [`ProbeReport::cleanup`] when it is no longer needed.
pub fn probe() -> Result<ProbeReport, GitProbeError> {
    let root = unique_root();
    probe_in(&root)
}

/// Runs the probe inside `root`, creating it when missing.
pub fn probe_in(root: &Path) -> Result<ProbeReport, GitProbeError> {
    fs::create_dir_all(root).map_err(|source| GitProbeError::Io {
        step: "create probe root",
        source,
    })?;

    let bare_repo = root.join("origin.git");
    let work_tree = root.join("work");

    let mut init = Command::new("git");
    init.env("GIT_TERMINAL_PROMPT", "0")
        .arg("init")
        .arg("--bare")
        .arg("--initial-branch")
        .arg(PROBE_BRANCH)
        .arg(&bare_repo);
    run("git init --bare", init)?;

    let mut clone = Command::new("git");
    clone
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("clone")
        .arg(&bare_repo)
        .arg(&work_tree);
    run("git clone", clone)?;

    let probe_file = work_tree.join(PROBE_FILE);
    fs::write(
        &probe_file,
        format!("musehub git probe\nversion={}\n", musehub_core::version()),
    )
    .map_err(|source| GitProbeError::Io {
        step: "write probe file",
        source,
    })?;

    let mut add = Command::new("git");
    add.arg("-C").arg(&work_tree).arg("add").arg("--all");
    run("git add --all", add)?;

    let mut commit = Command::new("git");
    commit
        .arg("-C")
        .arg(&work_tree)
        .arg("-c")
        .arg(format!("user.email={PROBE_EMAIL}"))
        .arg("-c")
        .arg(format!("user.name={PROBE_USER}"))
        .arg("commit")
        .arg("--no-gpg-sign")
        .arg("-m")
        .arg(PROBE_MESSAGE);
    run("git commit", commit)?;

    let mut push = Command::new("git");
    push.arg("-C")
        .arg(&work_tree)
        .arg("push")
        .arg("origin")
        .arg(format!("HEAD:refs/heads/{PROBE_BRANCH}"));
    run("git push", push)?;

    let commit_oid = git_dir_stdout(
        &bare_repo,
        "git rev-parse HEAD^commit",
        &["rev-parse", "--verify", "HEAD^{commit}"],
    )?;
    let tree_oid = git_dir_stdout(
        &bare_repo,
        "git rev-parse HEAD^tree",
        &["rev-parse", "--verify", "HEAD^{tree}"],
    )?;
    let listing = git_dir_stdout(&bare_repo, "git ls-tree", &["ls-tree", "-r", "HEAD"])?;
    let tree_entries: Vec<String> = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();

    if commit_oid.is_empty() {
        return Err(GitProbeError::Verification {
            detail: "bare repository HEAD does not resolve to a commit".to_string(),
        });
    }
    if tree_oid.is_empty() {
        return Err(GitProbeError::Verification {
            detail: "bare repository HEAD has no tree".to_string(),
        });
    }
    if tree_entries.is_empty() {
        return Err(GitProbeError::Verification {
            detail: "bare repository HEAD tree is empty".to_string(),
        });
    }

    let remote_head = git_dir_stdout(
        &bare_repo,
        "git symbolic-ref HEAD",
        &["symbolic-ref", "HEAD"],
    )?;
    let expected_head = format!("refs/heads/{PROBE_BRANCH}");
    if remote_head != expected_head {
        return Err(GitProbeError::Verification {
            detail: format!("bare repository HEAD is {remote_head}, expected {expected_head}"),
        });
    }

    Ok(ProbeReport {
        root: root.to_path_buf(),
        bare_repo,
        work_tree,
        commit: commit_oid,
        tree: tree_oid,
        tree_entries,
        remote_head,
    })
}

fn unique_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("musehub-git-probe-{}-{nanos}", std::process::id()))
}

fn git_dir_stdout(
    bare_repo: &Path,
    step: &'static str,
    args: &[&str],
) -> Result<String, GitProbeError> {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("--git-dir")
        .arg(bare_repo);
    command.args(args);
    run(step, command)
}

fn run(step: &'static str, mut command: Command) -> Result<String, GitProbeError> {
    let output = command
        .output()
        .map_err(|source| GitProbeError::Io { step, source })?;
    if !output.status.success() {
        return Err(GitProbeError::Failed {
            step,
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Git probe failures. Every variant maps to the `git_probe_failed` code.
#[derive(Debug, thiserror::Error)]
pub enum GitProbeError {
    #[error("git probe step `{step}` failed ({status}): {stderr}")]
    Failed {
        step: &'static str,
        status: String,
        stderr: String,
    },
    #[error("git probe step `{step}` could not run: {source}")]
    Io {
        step: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("git probe verification failed: {detail}")]
    Verification { detail: String },
}

impl ErrorCode for GitProbeError {
    fn code(&self) -> &'static str {
        "git_probe_failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_is_stable() {
        let error = GitProbeError::Verification {
            detail: "no tree".to_string(),
        };
        assert_eq!(error.code(), "git_probe_failed");
        assert!(error.to_string().contains("no tree"));
    }

    #[test]
    fn unreachable_bare_path_reports_git_stderr() {
        let root = unique_root();
        fs::create_dir_all(&root).expect("temp root");
        fs::write(root.join("origin.git"), b"not a directory").expect("blocking file");

        let error = probe_in(&root).expect_err("init must fail when origin.git is a file");
        let message = error.to_string();
        assert_eq!(error.code(), "git_probe_failed");
        assert!(message.contains("git init --bare"), "{message}");
        assert!(message.contains("fatal"), "{message}");

        let _ = fs::remove_dir_all(&root);
    }
}
