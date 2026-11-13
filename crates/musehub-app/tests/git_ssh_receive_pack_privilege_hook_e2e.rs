//! receive-pack child privilege: probe via pre-receive hook (real setcap gateway).

mod common;

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::{git_ssh_cmd_in_bounded, require_git_ssh_transport_prereqs};

fn install_privilege_probe_hook(bare: &std::path::Path, probe_path: &std::path::Path) {
    let hooks = bare.join("hooks");
    std::fs::create_dir_all(&hooks).expect("hooks dir");
    let hook = hooks.join("pre-receive");
    let probe_s = probe_path.to_string_lossy().replace('\'', "'\\''");
    let probe = format!("'{probe_s}'");
    let script = format!(
        r#"#!/bin/sh
grep -E '^(CapEff|CapPrm|CapAmb|Gid|Groups):' /proc/self/status >{probe}
protected="refs/heads/main"
while read -r old new ref; do
  case "$ref" in
    refs/heads/agent/*) ;;
    refs/tags/*)
      echo "refusing direct push to tag ref" 1>&2
      exit 1
      ;;
    "$protected")
      echo "refusing direct push to protected branch main" 1>&2
      exit 1
      ;;
  esac
done
exit 0
"#
    );
    std::fs::write(&hook, script).expect("write hook");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("chmod hook");
}

#[tokio::test]
async fn receive_pack_hook_sees_zero_caps_and_storage_gid() {
    require_git_ssh_transport_prereqs();
    let gid_line = Command::new("getent")
        .args(["group", "musehub-git"])
        .output()
        .expect("getent");
    assert!(gid_line.status.success(), "musehub-git group required");
    let musehub_gid = String::from_utf8_lossy(&gid_line.stdout)
        .split(':')
        .nth(2)
        .unwrap_or("")
        .trim()
        .to_string();

    let f = git_ssh_fixture("privhook").await;
    let storage_key: String =
        sqlx::query_scalar("SELECT storage_key::text FROM repositories WHERE slug = $1")
            .bind(&f.repo)
            .fetch_one(&f.pool)
            .await
            .expect("storage_key");
    let bare = f.git_root.join(format!("{storage_key}.git"));
    let probe_path = std::env::temp_dir().join(format!(
        "musehub-priv-hook-{}-{}",
        std::process::id(),
        f.repo
    ));
    let _ = std::fs::remove_file(&probe_path);
    install_privilege_probe_hook(&bare, &probe_path);

    let work_dir = tempfile::tempdir().expect("work");
    let work = work_dir.path();
    let clone = common::git_ssh_util::git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, work.to_str().unwrap()],
    );
    assert!(
        clone.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    std::fs::write(work.join("priv.txt"), b"x\n").unwrap();
    Command::new("git")
        .current_dir(work)
        .args(["checkout", "-b", "feature/priv-hook"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(work)
        .args(["add", "priv.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(work)
        .args([
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "priv",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();

    let push = git_ssh_cmd_in_bounded(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "origin", "HEAD:refs/heads/feature/priv-hook"],
    );
    let stderr = String::from_utf8_lossy(&push.stderr);
    assert!(
        !stderr.contains(bare.to_string_lossy().as_ref()),
        "client stderr must not contain bare repo path"
    );
    assert!(push.status.success(), "push failed: {stderr}");
    let body = std::fs::read_to_string(&probe_path).expect("privilege probe output");
    assert!(
        body.contains("CapEff:\t0000000000000000"),
        "CapEff must be zero, got:\n{body}"
    );
    assert!(
        body.contains("CapPrm:\t0000000000000000"),
        "CapPrm must be zero"
    );
    assert!(
        body.contains("CapAmb:\t0000000000000000"),
        "CapAmb must be zero"
    );
    let gid_line = body
        .lines()
        .find(|l| l.starts_with("Gid:"))
        .expect("Gid line in probe");
    let gid_parts: Vec<&str> = gid_line.split_whitespace().collect();
    assert_eq!(
        gid_parts.len(),
        5,
        "Gid line needs label + real/effective/saved/fs: {gid_line}"
    );
    assert!(
        gid_parts[1..5].iter().all(|&g| g == musehub_gid.as_str()),
        "Gid real/effective/saved/fs must all be storage gid {musehub_gid}: {gid_line}"
    );
    let groups_line = body
        .lines()
        .find(|l| l.starts_with("Groups:"))
        .expect("Groups line in probe");
    let groups_field = groups_line.strip_prefix("Groups:").unwrap_or("").trim();
    assert_eq!(
        groups_field,
        musehub_gid.as_str(),
        "Groups must be only storage gid: {groups_line}"
    );
    let _ = std::fs::remove_file(probe_path);
}
