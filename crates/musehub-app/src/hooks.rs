use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::error::AppError;

/// Installs a `pre-receive` hook protecting the default branch and internal refs.
///
/// The empty repository seed commit may be replaced exactly once as the initial mirror import.
/// Git runs this hook for client pushes over smart HTTP/SSH; server-side ref updates bypass it.
pub fn install_pre_receive_hook(bare: &Path, default_branch: &str) -> Result<(), AppError> {
    let hooks = bare.join("hooks");
    fs::create_dir_all(&hooks).map_err(|e| AppError::Other(e.to_string()))?;
    let hook_path = hooks.join("pre-receive");
    let script = format!(
        r#"#!/bin/sh
# Installed by MuseHub — protects the default branch and keeps tags immutable.
protected="refs/heads/{default_branch}"
seed="$(git config --local --get musehub.emptyRepoSeedOid 2>/dev/null || true)"
zero="0000000000000000000000000000000000000000"
while read -r old new ref; do
  case "$ref" in
    refs/musehub/*)
      echo "refusing update to platform-internal refs" 1>&2
      exit 1
      ;;
    refs/heads/agent/*) ;;
    refs/tags/*)
      case "$old" in
        *[!0]*)
          echo "refusing tag update or deletion; tags are immutable" 1>&2
          exit 1
          ;;
      esac
      ;;
    "$protected")
      if [ -n "$seed" ] && [ "$old" = "$seed" ] && [ "$new" != "$zero" ]; then
        :
      else
        echo "refusing direct push to protected branch {default_branch}" 1>&2
        exit 1
      fi
      ;;
  esac
done
exit 0
"#
    );
    if fs::read(&hook_path).is_ok_and(|current| current == script.as_bytes()) {
        return Ok(());
    }
    let mut file = tempfile::NamedTempFile::new_in(&hooks)
        .map_err(|error| AppError::Other(format!("create temporary Git hook: {error}")))?;
    file.write_all(script.as_bytes())
        .map_err(|error| AppError::Other(format!("write Git hook: {error}")))?;
    let status = Command::new("chmod")
        .args(["0755", file.path().to_str().expect("utf8")])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !status.success() {
        return Err(AppError::Other(
            "could not make pre-receive hook executable".into(),
        ));
    }
    file.persist(&hook_path).map_err(|error| {
        AppError::Other(format!("install Git hook atomically: {}", error.error))
    })?;
    Ok(())
}

/// Refreshes receive and upload policy so existing repositories gain the same boundary.
pub fn ensure_bare_repo_policy(bare: &Path) -> Result<(), AppError> {
    crate::git_ssh_runtime::reject_symlink_path(bare)?;
    if !bare.is_dir() {
        return Err(AppError::Other(
            "bare Git repository is not a directory".into(),
        ));
    }
    for key in [
        "receive.hideRefs",
        "uploadpack.hideRefs",
        "transfer.hideRefs",
    ] {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(bare)
            .args(["config", "--local", "--replace-all", key, "refs/musehub"])
            .output()
            .map_err(|error| AppError::Other(format!("configure {key}: {error}")))?;
        if !output.status.success() {
            return Err(AppError::Other(format!(
                "configure {key}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    install_pre_receive_hook(bare, "main")
}
