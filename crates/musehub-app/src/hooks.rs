use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::error::AppError;

/// Installs a `pre-receive` hook denying direct pushes to the default branch.
///
/// Git runs `pre-receive` only for `git-receive-pack` (client `git push` over smart HTTP/SSH).
/// Server-side `git update-ref` and `git fetch` into the bare repo do not invoke this hook.
pub fn install_pre_receive_hook(bare: &Path, default_branch: &str) -> Result<(), AppError> {
    let hooks = bare.join("hooks");
    fs::create_dir_all(&hooks).map_err(|e| AppError::Other(e.to_string()))?;
    let hook_path = hooks.join("pre-receive");
    let script = format!(
        r#"#!/bin/sh
# Installed by MuseHub — blocks direct pushes to protected default branch.
protected="refs/heads/{default_branch}"
while read -r old new ref; do
  case "$ref" in
    refs/heads/agent/*) ;;
    "$protected")
      echo "refusing direct push to protected branch {default_branch}" 1>&2
      exit 1
      ;;
  esac
done
exit 0
"#
    );
    let mut file = fs::File::create(&hook_path).map_err(|e| AppError::Other(e.to_string()))?;
    file.write_all(script.as_bytes())
        .map_err(|e| AppError::Other(e.to_string()))?;
    Command::new("chmod")
        .args(["0755", hook_path.to_str().expect("utf8")])
        .status()
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}
