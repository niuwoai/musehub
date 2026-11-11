//! Product Git-over-SSH: AuthorizedKeysCommand, gateway, optional sshd supervisor.

use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use musehub_app::git_ssh::{
    self, lookup_ssh_key_by_fingerprint, reload_ssh_key_session, render_sshd_config,
    write_authorized_keys_stdout,
};
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
        "gateway" => gateway().await,
        "serve" => serve(args.get(1).map(String::as_str)).await,
        _ => {
            eprintln!("usage: musehub-git-ssh {{authorized-keys|gateway|serve}}");
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
    PathBuf::from(env::var("MUSEHUB_GIT_ROOT").unwrap_or_else(|_| "/tmp/musehub-git".into()))
}

fn gateway_bin() -> PathBuf {
    env::var("MUSEHUB_GIT_SSH_GATEWAY_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_exe().unwrap_or_else(|_| PathBuf::from("musehub-git-ssh")))
}

async fn pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(PgPoolOptions::new()
        .max_connections(2)
        .connect(&require_database_url()?)
        .await?)
}

async fn authorized_keys(args: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fingerprint = args.last().ok_or("missing fingerprint arg")?;
    let pool = pool().await?;
    match lookup_ssh_key_by_fingerprint(&pool, fingerprint).await {
        Ok(auth) => {
            write_authorized_keys_stdout(&auth, &gateway_bin().display().to_string())?;
            Ok(())
        }
        Err(_) => std::process::exit(1),
    }
}

async fn gateway() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let key_id = env::var("MUSEHUB_SSH_KEY_ID")
        .map_err(|_| "missing MUSEHUB_SSH_KEY_ID")?
        .parse()?;
    let principal_id = env::var("MUSEHUB_SSH_PRINCIPAL_ID")
        .map_err(|_| "missing MUSEHUB_SSH_PRINCIPAL_ID")?
        .parse()?;
    let original = env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    let pool = pool().await?;
    let _ = reload_ssh_key_session(&pool, key_id, principal_id).await?;
    match git_ssh::run_ssh_gateway(&pool, &require_git_root(), key_id, principal_id, &original)
        .await
    {
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
    let run_dir = PathBuf::from(
        env::var("MUSEHUB_GIT_SSH_RUN_DIR").unwrap_or_else(|_| "/tmp/musehub-git-ssh".into()),
    );
    std::fs::create_dir_all(&run_dir)?;
    let host_key = run_dir.join("ssh_host_ed25519_key");
    if !host_key.exists() {
        let status = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                host_key.to_str().expect("path"),
                "-N",
                "",
                "-q",
            ])
            .status()?;
        if !status.success() {
            return Err("ssh-keygen host key failed".into());
        }
    }
    let pid_file = run_dir.join("sshd.pid");
    let exe = env::current_exe()?;
    let config_body = render_sshd_config(port, &host_key, &pid_file, &exe);
    let config_path = run_dir.join("sshd_config");
    std::fs::write(&config_path, config_body)?;
    tracing::info!(port, ?config_path, "starting product git sshd");
    let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
        .map(PathBuf::from)
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("sshd"));
    let mut child = Command::new(sshd);
    child.arg("-D").arg("-f").arg(&config_path).arg("-e");
    child.stdin(Stdio::null());
    child.stdout(Stdio::inherit());
    child.stderr(Stdio::inherit());
    let status = child.status()?;
    std::process::exit(status.code().unwrap_or(1));
}
