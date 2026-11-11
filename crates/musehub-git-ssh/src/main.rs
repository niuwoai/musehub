//! Product Git-over-SSH: AuthorizedKeysCommand, gateway, optional sshd supervisor.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use musehub_app::git_ssh::{
    self, assert_git_ssh_connection_user, lookup_ssh_key_by_fingerprint, reload_ssh_key_by_id,
    render_sshd_config, write_authorized_keys_stdout, GIT_SSH_LOGIN_USER,
};
use musehub_app::git_ssh_runtime::{
    default_git_ssh_run_dir, ensure_host_ed25519_key, prepare_git_ssh_run_dir,
    write_git_ssh_runtime_env, write_secure_file,
};
use musehub_app::ssh_keys::fingerprint_public_key_canonical;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    let sub = args.first().map(|s| s.as_str()).unwrap_or("help");
    match sub {
        "authorized-keys" => authorized_keys(&args[1..]).await,
        "gateway" | "shell" => gateway().await,
        "serve" => serve(args.get(1).map(String::as_str)).await,
        _ => {
            eprintln!("usage: musehub-git-ssh {{authorized-keys|gateway|shell|serve}}");
            std::process::exit(2);
        }
    }
}

fn require_database_url() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let url = env::var("DATABASE_URL")
        .or_else(|_| env::var("MUSEHUB_TEST_DATABASE_URL"))
        .map_err(|_| "DATABASE_URL required")?;
    Ok(url)
}

fn require_git_root() -> PathBuf {
    PathBuf::from(env::var("MUSEHUB_GIT_ROOT").unwrap_or_else(|_| "/var/lib/musehub/git".into()))
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn gateway_bin() -> PathBuf {
    env::var("MUSEHUB_GIT_SSH_GATEWAY_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_exe().unwrap_or_else(|_| PathBuf::from("musehub-git-ssh")))
}

fn git_ssh_akc_user() -> String {
    env::var("MUSEHUB_GIT_SSH_AKC_USER").unwrap_or_else(|_| GIT_SSH_LOGIN_USER.to_string())
}

const GIT_LOGIN_SHELL: &str = "/usr/local/lib/musehub/musehub-git-login.sh";

const DEFAULT_RUNTIME_ENV: &str = "/var/lib/musehub/git-ssh/runtime.env";

fn install_git_login_shell() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let script = format!(
        r#"#!/bin/sh
RUNTIME_ENV="${{MUSEHUB_GIT_SSH_RUNTIME_ENV:-{default_env}}}"
if [ -r "$RUNTIME_ENV" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$RUNTIME_ENV"
  set +a
fi
if [ -z "${{SSH_ORIGINAL_COMMAND:-}}" ] && [ "$#" -ge 2 ]; then
  export SSH_ORIGINAL_COMMAND="$1 '$2'"
fi
exec "${{MUSEHUB_GIT_SSH_GATEWAY_BIN:-/usr/local/bin/musehub-git-ssh}}" shell
"#,
        default_env = DEFAULT_RUNTIME_ENV,
    );
    let install_dir = Path::new("/usr/local/lib/musehub");
    if !install_dir.exists() {
        let _ = Command::new("sudo")
            .args(["install", "-d", "-m", "0755", "-o", "root", "-g", "root"])
            .arg(install_dir)
            .status();
    }
    let tmp = env::temp_dir().join("musehub-git-login.sh");
    std::fs::write(&tmp, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    let status = Command::new("sudo")
        .args([
            "install",
            "-m",
            "0755",
            "-o",
            "root",
            "-g",
            "root",
            tmp.to_str().expect("tmp"),
            GIT_LOGIN_SHELL,
        ])
        .status()?;
    if !status.success() {
        return Err("install git login shell".into());
    }
    let _ = Command::new("sudo")
        .args(["usermod", "-s", GIT_LOGIN_SHELL, GIT_SSH_LOGIN_USER])
        .status();
    let _ = Command::new("sudo")
        .args([
            "bash",
            "-c",
            &format!(
                "grep -qxF {shell} /etc/shells 2>/dev/null || echo {shell} >> /etc/shells",
                shell = shell_single_quote(GIT_LOGIN_SHELL)
            ),
        ])
        .status();
    Ok(())
}

#[cfg(unix)]
fn chown_root_if_possible(path: &Path) -> bool {
    if std::env::var("MUSEHUB_GIT_SSH_SUDO_SSHD").ok().as_deref() == Some("1") {
        return Command::new("sudo")
            .args(["chown", "root:root", path.to_str().unwrap_or("")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    // In container/systemd deployments the serve process is root; chown is implicit.
    false
}

#[cfg(not(unix))]
fn chown_root_if_possible(_path: &Path) -> bool {
    false
}

async fn pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(PgPoolOptions::new()
        .max_connections(2)
        .connect(&require_database_url()?)
        .await?)
}

async fn authorized_keys(args: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ssh_user = args.first().ok_or("missing ssh user arg")?;
    assert_git_ssh_connection_user(ssh_user).map_err(|_| "invalid ssh user")?;
    let fingerprint = args.last().ok_or("missing fingerprint arg")?;
    let derived_fp = if args.len() >= 4 {
        let key_b64 = &args[1];
        let key_type = &args[2];
        fingerprint_public_key_canonical(&format!("{key_type} {key_b64}"))
            .ok()
            .or_else(|| Some(fingerprint.clone()))
    } else {
        None
    };
    let pool = pool().await?;
    let lookup_fp = derived_fp.as_deref().unwrap_or(fingerprint.as_str());
    let auth = match lookup_ssh_key_by_fingerprint(&pool, lookup_fp).await {
        Ok(auth) => auth,
        Err(_) if lookup_fp != fingerprint.as_str() => {
            lookup_ssh_key_by_fingerprint(&pool, fingerprint)
                .await
                .map_err(|_| "unknown key")?
        }
        Err(_) => {
            std::process::exit(1);
        }
    };
    write_authorized_keys_stdout(&auth, &gateway_bin().display().to_string())?;
    Ok(())
}

async fn gateway() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let key_id = env::var("MUSEHUB_SSH_KEY_ID")
        .map_err(|_| "missing MUSEHUB_SSH_KEY_ID")?
        .parse()?;
    let original = env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    let pool = pool().await?;
    let _ = reload_ssh_key_by_id(&pool, key_id).await?;
    match git_ssh::run_ssh_gateway(&pool, &require_git_root(), key_id, &original).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

async fn serve(port_arg: Option<&str>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = port_arg
        .and_then(|p| p.parse().ok())
        .or_else(|| {
            env::var("MUSEHUB_GIT_SSH_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
        })
        .unwrap_or(2222);
    let run_dir = default_git_ssh_run_dir();
    prepare_git_ssh_run_dir(&run_dir).map_err(|e| e.to_string())?;
    let host_key = run_dir.join("ssh_host_ed25519_key");
    ensure_host_ed25519_key(&host_key).map_err(|e| e.to_string())?;
    let pid_file = run_dir.join("sshd.pid");
    let exe = env::current_exe()?;
    let staged_bin = run_dir.join("musehub-git-ssh");
    if !staged_bin.exists() {
        std::fs::copy(&exe, &staged_bin)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged_bin, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    let db_url = require_database_url()?;
    let git_root = require_git_root();
    let gateway = gateway_bin();
    let git_root_s = git_root.to_str().unwrap_or("/var/lib/musehub/git");
    let gateway_s = gateway.to_str().unwrap_or("musehub-git-ssh");
    let akc_user = git_ssh_akc_user();
    install_git_login_shell()?;
    let runtime_env_path = write_git_ssh_runtime_env(
        &run_dir,
        &[
            ("DATABASE_URL", &db_url),
            ("MUSEHUB_GIT_ROOT", git_root_s),
            ("MUSEHUB_GIT_SSH_GATEWAY_BIN", gateway_s),
        ],
    )
    .map_err(|e| e.to_string())?;
    let runtime_env_s = runtime_env_path
        .to_str()
        .ok_or("runtime env path")?
        .to_string();
    let akc_wrap = run_dir.join("akc-wrap.sh");
    let akc_script = r#"#!/bin/sh
RUNTIME_ENV="${MUSEHUB_GIT_SSH_RUNTIME_ENV:-$(dirname "$0")/runtime.env}"
if [ -r "$RUNTIME_ENV" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$RUNTIME_ENV"
  set +a
fi
exec "${MUSEHUB_GIT_SSH_GATEWAY_BIN:-/usr/local/bin/musehub-git-ssh}" authorized-keys "$@"
"#
    .to_string();
    std::fs::write(&akc_wrap, akc_script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // OpenSSH requires root ownership and no group/other write; the AKC identity
        // (`AuthorizedKeysCommandUser`, default `git`) must be able to execute the script.
        std::fs::set_permissions(&akc_wrap, std::fs::Permissions::from_mode(0o755))?;
        let _ = chown_root_if_possible(&akc_wrap);
    }
    let setenv: Vec<(&str, &str)> = vec![("MUSEHUB_GIT_SSH_RUNTIME_ENV", runtime_env_s.as_str())];
    let use_wrap = std::env::var("MUSEHUB_GIT_SSH_AKC_WRAP").ok().as_deref() != Some("0");
    let system_bin = PathBuf::from("/usr/local/bin/musehub-git-ssh");
    let akc_bin: &Path = if !use_wrap && system_bin.exists() {
        &system_bin
    } else if use_wrap {
        &akc_wrap
    } else {
        &staged_bin
    };
    let config_body = render_sshd_config(
        port,
        &host_key,
        &pid_file,
        akc_bin,
        &setenv,
        GIT_SSH_LOGIN_USER,
        &akc_user,
        use_wrap,
    );
    let config_path = run_dir.join("sshd_config");
    write_secure_file(&config_path, config_body.as_bytes())?;
    tracing::info!(port, ?config_path, "starting product git sshd");
    let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
        .map(PathBuf::from)
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("sshd"));
    let mut child = if std::env::var("MUSEHUB_GIT_SSH_SUDO_SSHD").ok().as_deref() == Some("1") {
        let mut c = Command::new("sudo");
        c.arg("-n").arg(&sshd);
        c
    } else {
        Command::new(&sshd)
    };
    child.arg("-D").arg("-f").arg(&config_path).arg("-e");
    child.stdin(Stdio::null());
    child.stdout(Stdio::inherit());
    child.stderr(Stdio::inherit());
    let status = child.status()?;
    std::process::exit(status.code().unwrap_or(1));
}
