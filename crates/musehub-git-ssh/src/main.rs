//! Product Git-over-SSH: AuthorizedKeysCommand, gateway, optional sshd supervisor.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use musehub_app::git_ssh::{
    self, assert_git_ssh_connection_user, lookup_ssh_key_by_fingerprint, render_sshd_config,
    write_authorized_keys_stdout, GIT_SSH_LOGIN_USER,
};
use musehub_app::git_ssh_runtime::{
    default_git_ssh_run_dir, ensure_host_ed25519_key, ensure_shared_git_storage_root,
    finalize_git_ssh_run_dir, git_ssh_runtime_group, git_storage_group, is_root_euid,
    prepare_git_ssh_run_dir, run_privileged, validate_git_ssh_runtime_readable,
    write_git_ssh_runtime_env, write_secure_file,
};
use musehub_app::ssh_keys::fingerprint_public_key_canonical;
use sqlx::postgres::PgPoolOptions;
use std::fs::OpenOptions;
use std::io::Write;
use tracing_subscriber::EnvFilter;

const SERVER_LOG_ENV: &str = "MUSEHUB_GIT_SSH_SERVER_LOG";

struct ServerTracingWriter;

impl Write for ServerTracingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(path) = env::var(SERVER_LOG_ENV) {
            let path = path.trim();
            if !path.is_empty() {
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                    return f.write(buf);
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// SSH forced-command modes must not attach tracing to stderr (client-visible).
fn init_tracing_for_subcommand(sub: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let ssh_client_mode = matches!(sub, "gateway" | "shell" | "authorized-keys");
    if ssh_client_mode {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_writer(|| ServerTracingWriter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .init();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let sub = args.first().map(|s| s.as_str()).unwrap_or("help");
    init_tracing_for_subcommand(sub);
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

const PINNED_GATEWAY_BIN: &str = "/usr/local/bin/musehub-git-ssh";

fn pinned_gateway_bin() -> PathBuf {
    PathBuf::from(PINNED_GATEWAY_BIN)
}

fn gateway_bin() -> PathBuf {
    if env::var("MUSEHUB_GIT_SSH_ALLOW_GATEWAY_BIN_ENV")
        .ok()
        .as_deref()
        == Some("1")
    {
        env::var("MUSEHUB_GIT_SSH_GATEWAY_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| pinned_gateway_bin())
    } else {
        pinned_gateway_bin()
    }
}

fn git_ssh_akc_user() -> String {
    env::var("MUSEHUB_GIT_SSH_AKC_USER").unwrap_or_else(|_| GIT_SSH_LOGIN_USER.to_string())
}

#[cfg(unix)]
fn validate_cap_setgid_target(bin: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(bin).map_err(|e| e.to_string())?;
    if meta.uid() != 0 {
        return Err(format!(
            "{} must be root-owned before setcap",
            bin.display()
        ));
    }
    let mode = meta.mode() & 0o777;
    if mode & 0o022 != 0 {
        return Err(format!(
            "{} must not be group/world writable before setcap",
            bin.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn apply_cap_setgid_on_gateway(bin: &Path) -> Result<(), String> {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let meta_before = fs::metadata(bin).map_err(|e| e.to_string())?;
    let ino_before = meta_before.ino();
    validate_cap_setgid_target(bin)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(bin)
        .map_err(|e| e.to_string())?;
    let fd = file.as_raw_fd();
    let mut st_fd = std::mem::MaybeUninit::<libc::stat>::zeroed();
    if unsafe { libc::fstat(fd, st_fd.as_mut_ptr()) } != 0 {
        return Err("fstat staged gateway failed".into());
    }
    let st_fd = unsafe { st_fd.assume_init() };
    if st_fd.st_ino != ino_before {
        return Err("gateway path inode changed before setcap".into());
    }
    let path = bin.to_str().ok_or("gateway binary path")?;
    musehub_app::git_ssh_runtime::run_privileged("setcap", &["cap_setgid+ep", path])
        .map_err(|e| format!("setcap cap_setgid+ep on {path}: {e}"))?;
    let meta_after = fs::metadata(bin).map_err(|e| e.to_string())?;
    if meta_after.ino() != ino_before || meta_after.uid() != 0 {
        return Err("gateway binary changed after setcap".into());
    }
    validate_cap_setgid_target(bin)
}

#[cfg(unix)]
fn harden_staged_gateway_copy(staged: &Path) -> Result<(), String> {
    let staged_s = staged.to_str().ok_or("staged gateway path")?;
    if !is_root_euid() {
        run_privileged("chown", &["root:root", staged_s]).map_err(|e| e.to_string())?;
        run_privileged("chmod", &["755", staged_s]).map_err(|e| e.to_string())?;
    } else {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    validate_cap_setgid_target(staged)
}

const GIT_LOGIN_SHELL: &str = "/usr/local/lib/musehub/musehub-git-login.sh";

const DEFAULT_RUNTIME_ENV: &str = "/var/lib/musehub/git-ssh/runtime.env";

fn install_git_login_shell() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let script = format!(
        r#"#!/bin/sh
RUNTIME_ENV="${{MUSEHUB_GIT_SSH_RUNTIME_ENV:-{default_env}}}"
if [ ! -r "$RUNTIME_ENV" ]; then
  echo "missing runtime env" >&2
  exit 1
fi
set -a
# shellcheck disable=SC1090
. "$RUNTIME_ENV"
set +a
if [ -z "${{SSH_ORIGINAL_COMMAND:-}}" ] && [ "$#" -ge 2 ]; then
  export SSH_ORIGINAL_COMMAND="$1 '$2'"
fi
exec /usr/local/bin/musehub-git-ssh shell
"#,
        default_env = DEFAULT_RUNTIME_ENV,
    );
    let install_dir = Path::new("/usr/local/lib/musehub");
    if !install_dir.exists() {
        if is_root_euid() {
            std::fs::create_dir_all(install_dir).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(install_dir, std::fs::Permissions::from_mode(0o755))?;
            }
        } else {
            run_privileged(
                "install",
                &[
                    "-d",
                    "-m",
                    "0755",
                    "-o",
                    "root",
                    "-g",
                    "root",
                    install_dir.to_str().expect("dir"),
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let tmp = env::temp_dir().join("musehub-git-login.sh");
    std::fs::write(&tmp, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    if is_root_euid() {
        if let Some(parent) = Path::new(GIT_LOGIN_SHELL).parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&tmp, GIT_LOGIN_SHELL).map_err(|e| e.to_string())?;
    } else {
        run_privileged(
            "install",
            &[
                "-m",
                "0755",
                "-o",
                "root",
                "-g",
                "root",
                tmp.to_str().expect("tmp"),
                GIT_LOGIN_SHELL,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    run_privileged("usermod", &["-s", GIT_LOGIN_SHELL, GIT_SSH_LOGIN_USER])
        .map_err(|e| e.to_string())?;
    let shells_cmd = format!(
        "grep -qxF {shell} /etc/shells 2>/dev/null || echo {shell} >> /etc/shells",
        shell = shell_single_quote(GIT_LOGIN_SHELL)
    );
    if is_root_euid() {
        if !std::fs::read_to_string("/etc/shells")
            .map(|s| s.lines().any(|l| l == GIT_LOGIN_SHELL))
            .unwrap_or(false)
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open("/etc/shells")
                .map_err(|e| e.to_string())?;
            writeln!(f, "{GIT_LOGIN_SHELL}").map_err(|e| e.to_string())?;
        }
    } else {
        run_privileged("bash", &["-c", &shells_cmd]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(PgPoolOptions::new()
        .max_connections(4)
        .connect(&require_database_url()?)
        .await?)
}

async fn authorized_keys(args: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(unix)]
    musehub_app::git_ssh_pack_privilege::drop_gateway_inherited_capabilities()
        .map_err(|e| e.to_string())?;
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
    write_authorized_keys_stdout(&auth, &gateway_bin().display().to_string(), lookup_fp)?;
    Ok(())
}

async fn gateway() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Client-supplied env cannot choose gateway binary or database URL when unset in login shell.
    env::remove_var("MUSEHUB_GIT_SSH_GATEWAY_BIN");
    let original = env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    let pool = pool().await?;
    let session_fp = git_ssh::session_akc_bound_pubkey_fingerprint()
        .map_err(|_| "missing session key fingerprint")?;
    let auth = lookup_ssh_key_by_fingerprint(&pool, &session_fp)
        .await
        .map_err(|_| "unknown key")?;
    let plan = git_ssh::plan_ssh_pack_gateway(&pool, &require_git_root(), auth.key_id, &original)
        .await
        .map_err(|e| {
            musehub_app::git_ssh_server_log::log_pack_plan_failure(&e.to_string());
            "pack operation failed"
        })?;
    // Release DB handles before blocking pack I/O (do not await graceful close — can stall).
    drop(pool);
    match git_ssh::execute_ssh_pack(&plan) {
        Ok(code) => std::process::exit(code),
        Err(_) => {
            eprintln!("{}", git_ssh::GIT_PACK_CLIENT_FAILURE);
            std::process::exit(1);
        }
    }
}

async fn serve(port_arg: Option<&str>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(unix)]
    musehub_app::git_ssh_pack_privilege::drop_gateway_inherited_capabilities()
        .map_err(|e| e.to_string())?;
    let port: u16 = port_arg
        .and_then(|p| p.parse().ok())
        .or_else(|| {
            env::var("MUSEHUB_GIT_SSH_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
        })
        .unwrap_or(2222);
    let run_dir = default_git_ssh_run_dir();
    musehub_app::git_ssh_runtime::ensure_git_ssh_login_home(git_ssh::GIT_SSH_LOGIN_USER)
        .map_err(|e| e.to_string())?;
    musehub_app::git_ssh_runtime::verify_git_ssh_passwd_home(git_ssh::GIT_SSH_LOGIN_USER)
        .map_err(|e| e.to_string())?;
    musehub_app::git_ssh_runtime::verify_git_ssh_login_home().map_err(|e| e.to_string())?;
    ensure_shared_git_storage_root(&require_git_root()).map_err(|e| e.to_string())?;
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
    #[cfg(unix)]
    {
        harden_staged_gateway_copy(&staged_bin)?;
        if pinned_gateway_bin().exists() {
            apply_cap_setgid_on_gateway(&pinned_gateway_bin())?;
        }
        apply_cap_setgid_on_gateway(&staged_bin)?;
    }
    let db_url = require_database_url()?;
    let git_root = require_git_root();
    let _gateway = gateway_bin();
    let git_root_s = git_root.to_str().unwrap_or("/var/lib/musehub/git");
    let akc_user = git_ssh_akc_user();
    install_git_login_shell()?;
    let packaging_marker = env::var("MUSEHUB_GIT_SSH_PACKAGING_TEST_MARKER").ok();
    let packaging_synthetic = env::var("MUSEHUB_GIT_SSH_PACKAGING_SYNTHETIC_SECRET").ok();
    let storage_group = git_storage_group();
    let server_log = run_dir.join("musehub-git-ssh.log");
    let server_log_s = server_log.to_str().ok_or("server log path")?.to_string();
    let mut runtime_vars: Vec<(&str, &str)> = vec![
        ("DATABASE_URL", &db_url),
        ("MUSEHUB_GIT_ROOT", git_root_s),
        ("MUSEHUB_GIT_STORAGE_GROUP", storage_group.as_str()),
        ("MUSEHUB_GIT_SSH_SERVER_LOG", server_log_s.as_str()),
    ];
    if let Some(ref marker) = packaging_marker {
        if !marker.is_empty() {
            runtime_vars.push(("MUSEHUB_GIT_SSH_PACKAGING_TEST_MARKER", marker.as_str()));
        }
    }
    if let Some(ref secret) = packaging_synthetic {
        if !secret.is_empty() {
            runtime_vars.push((
                "MUSEHUB_GIT_SSH_PACKAGING_SYNTHETIC_SECRET",
                secret.as_str(),
            ));
        }
    }
    let runtime_env_path =
        write_git_ssh_runtime_env(&run_dir, &runtime_vars).map_err(|e| e.to_string())?;
    validate_git_ssh_runtime_readable(&run_dir).map_err(|e| e.to_string())?;
    let runtime_env_s = runtime_env_path
        .to_str()
        .ok_or("runtime env path")?
        .to_string();
    let akc_wrap = run_dir.join("akc-wrap.sh");
    let akc_script = r#"#!/bin/sh
RUNTIME_ENV="${MUSEHUB_GIT_SSH_RUNTIME_ENV:-$(dirname "$0")/runtime.env}"
if [ ! -r "$RUNTIME_ENV" ]; then
  exit 1
fi
set -a
# shellcheck disable=SC1090
. "$RUNTIME_ENV"
set +a
exec /usr/local/bin/musehub-git-ssh authorized-keys "$@"
"#
    .to_string();
    std::fs::write(&akc_wrap, akc_script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // OpenSSH requires root ownership and no group/other write; the AKC identity
        // (`AuthorizedKeysCommandUser`, default `git`) must be able to execute the script.
        std::fs::set_permissions(&akc_wrap, std::fs::Permissions::from_mode(0o750))?;
        musehub_app::git_ssh_runtime::chown_root_group(&akc_wrap, &git_ssh_runtime_group())
            .map_err(|e| e.to_string())?;
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
    finalize_git_ssh_run_dir(&run_dir).map_err(|e| e.to_string())?;
    validate_git_ssh_runtime_readable(&run_dir).map_err(|e| e.to_string())?;
    tracing::info!(port, ?config_path, "starting product git sshd");
    let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
        .map(PathBuf::from)
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("sshd"));
    let test = Command::new(&sshd)
        .arg("-t")
        .arg("-f")
        .arg(&config_path)
        .status()?;
    if !test.success() {
        return Err("sshd config test failed".into());
    }
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
