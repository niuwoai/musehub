//! OpenSSH forced-command probe executed inside the Colima Linux VM.
//!
//! The Rust gateway stays on the macOS host. It reaches the VM only through
//! `/opt/homebrew/bin/colima ssh` and argument arrays. The temporary `sshd`,
//! the `AuthorizedKeysCommand`, and the forced command are Python 3 from the
//! VM standard library, written over the remote standard input. Nothing is
//! compiled in the VM, and no real `git-upload-pack` or `git-receive-pack`
//! runs.
//!
//! # Scope
//!
//! Every report carries [`SCOPE`] (`linux_openssh_colima`). It proves, for one
//! temporary OpenSSH inside the Colima VM, that:
//!
//! * the principal comes from the server-side key options, not from the client
//!   command or a client `SetEnv`;
//! * only `git-upload-pack` and `git-receive-pack` with one logical path
//!   segment are accepted, and that segment is mapped to a fixed UUID path;
//! * two principals can succeed at the same time and each sees its own
//!   principal;
//! * disconnecting the client before the result line produces no repository
//!   file and no commit.
//!
//! This is a single-user Colima feasibility probe. It is not a production
//! multi-tenant isolation boundary, and it does not select an Agent runtime.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use musehub_core::ErrorCode;

/// Stable scope label included in every report.
pub const SCOPE: &str = "linux_openssh_colima";

/// Absolute path of the Colima CLI used to reach the Linux VM.
pub const COLIMA_BIN: &str = "/opt/homebrew/bin/colima";

/// Python 3 inside the Linux VM. The probe never looks it up through a shell.
pub const VM_PYTHON: &str = "/usr/bin/python3";

/// Temporary `sshd` inside the Linux VM.
pub const VM_SSHD: &str = "/usr/sbin/sshd";

/// Passwordless sudo used only to start and stop the temporary `sshd`.
pub const VM_SUDO: &str = "/usr/bin/sudo";

/// The only Git service commands the forced command accepts.
pub const ALLOWED_COMMANDS: [&str; 2] = ["git-upload-pack", "git-receive-pack"];

/// Fixed UUID storage path used for the logical repository `demo`.
pub const DEMO_UUID_PATH: &str =
    "repositories/00000000-0000-4000-8000-000000000001/11111111-1111-4111-8111-111111111111.git";

/// Logical repository name accepted by the probe.
pub const DEMO_LOGICAL: &str = "demo";

/// VM program that owns the temporary OpenSSH and answers the host gateway.
///
/// The host sends the program on standard input and reads one JSON line. It creates
/// its keys, configuration, and scripts under `~/.cache`, installs the
/// `AuthorizedKeysCommand` and forced command as root-owned files in `/run`,
/// and removes that `sshd` on the way out.
pub const VM_HARNESS: &str = r#"#!/usr/bin/python3
import json, os, pathlib, shutil, socket, subprocess, sys, time

HOME = pathlib.Path.home() / ".cache" / "musehub-ssh-probe"

def fail(message):
    sys.stdout.write(json.dumps({"ok": False, "error": message}) + "\n")
    sys.stdout.flush()
    sys.exit(0)

def run(argv):
    completed = subprocess.run(argv, capture_output=True, text=True)
    if completed.returncode != 0:
        fail("%s failed (%s): %s" % (" ".join(argv), completed.returncode, completed.stderr.strip()))
    return completed

def main():
    request = json.loads(os.environ["MUSEHUB_REQUEST"])
    action = request.get("action")
    if HOME.exists():
        shutil.rmtree(HOME)
    HOME.mkdir(parents=True, mode=0o755)
    HOME.chmod(0o755)
    run(["/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(HOME / "host")])
    principals = request.get("principals") or ["alice"]
    clients = {}
    for principal in principals:
        key = HOME / principal
        run(["/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key), "-C", principal])
        fingerprint = run(["/usr/bin/ssh-keygen", "-lf", str(key) + ".pub"]).stdout.split()[1]
        clients[fingerprint] = principal
    forced = HOME / "forced.py"
    forced.write_text(FORCED.replace("__CANCEL__", "True" if action == "cancel" else "False"))
    forced.chmod(0o755)
    public_keys = {principal: (HOME / (principal + ".pub")).read_text().strip() for principal in principals}
    akc = HOME / "akc.py"
    akc.write_text(AKC.replace("__CLIENTS__", json.dumps(clients)).replace("__PUBLIC_KEYS__", json.dumps(public_keys)))
    akc.chmod(0o755)
    installed_akc = "/run/musehub-ssh-akc.py"
    installed_forced = "/run/musehub-ssh-forced.py"
    for source, target in ((akc, installed_akc), (forced, installed_forced)):
        run(["/usr/bin/sudo", "-n", "/bin/cp", str(source), target])
        run(["/usr/bin/sudo", "-n", "/bin/chown", "root:root", target])
        run(["/usr/bin/sudo", "-n", "/bin/chmod", "755", target])
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    config = HOME / "sshd_config"
    config.write_text(CONFIG % (port, HOME / "host", HOME / "sshd.pid", installed_akc))
    log = HOME / "sshd.log"
    started = subprocess.run(
        ["/usr/bin/sudo", "-n", "/usr/sbin/sshd", "-f", str(config), "-E", str(log)],
        capture_output=True, text=True)
    if started.returncode != 0:
        fail("sshd failed (%s): %s" % (started.returncode, started.stderr.strip()))
    deadline = time.time() + 2
    while not (HOME / "sshd.pid").exists():
        if time.time() > deadline:
            copied = HOME / "sshd.log.copy"
            subprocess.run(["/usr/bin/sudo", "-n", "/bin/cp", str(log), str(copied)], check=False)
            subprocess.run(["/usr/bin/sudo", "-n", "/bin/chown", "rocky:rocky", str(copied)], check=False)
            detail = copied.read_text()[-1000:] if copied.exists() else started.stderr
            fail("sshd wrote no pid file: %s" % detail)
        time.sleep(0.05)
    results = []
    try:
        if action == "one":
            results.append(ssh(port, request["principal"], request["command"], request.get("setenv")))
        elif action == "concurrent":
            results.extend(concurrent(port, request["calls"]))
        elif action == "cancel":
            results.append(cancel(port, request["principal"], request["command"]))
        else:
            fail("unknown action %r" % action)

    finally:
        pid = (HOME / "sshd.pid").read_text().strip() if (HOME / "sshd.pid").exists() else ""
        if pid:
            subprocess.run(["/usr/bin/sudo", "-n", "/bin/kill", pid], check=False)
        subprocess.run(["/usr/bin/sudo", "-n", "/bin/rm", "-f", installed_akc, installed_forced], check=False)
    sys.stdout.write(json.dumps({"ok": True, "results": results, "scope": "linux_openssh_colima"}) + "\n")
    sys.stdout.flush()

def ssh(port, principal, command, setenv):
    argv = [
        "/usr/bin/ssh", "-i", str(HOME / principal),
        "-o", "UserKnownHostsFile=%s" % (HOME / "known_hosts"),
        "-o", "StrictHostKeyChecking=accept-new", "-o", "IdentitiesOnly=yes",
        "-o", "BatchMode=yes", "-o", "PreferredAuthentications=publickey",
        "-p", str(port), "rocky@127.0.0.1",
    ]
    if setenv:
        argv[1:1] = ["-o", "SetEnv=MUSEHUB_PRINCIPAL=%s" % setenv]
    argv.append(command)
    completed = subprocess.run(argv, capture_output=True, text=True)
    return {"exit_code": completed.returncode, "stdout": completed.stdout, "stderr": completed.stderr}

def concurrent(port, calls):
    children = []
    for call in calls:
        children.append(subprocess.Popen(ssh_argv(port, call["principal"], call["command"]), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True))
    out = []
    for child in children:
        stdout, stderr = child.communicate()
        out.append({"exit_code": child.returncode, "stdout": stdout, "stderr": stderr})
    return out

def ssh_argv(port, principal, command):
    return [
        "/usr/bin/ssh", "-i", str(HOME / principal),
        "-o", "UserKnownHostsFile=%s" % (HOME / "known_hosts"),
        "-o", "StrictHostKeyChecking=accept-new", "-o", "IdentitiesOnly=yes",
        "-o", "BatchMode=yes", "-p", str(port), "rocky@127.0.0.1", command,
    ]

def cancel(port, principal, command):
    child = subprocess.Popen(ssh_argv(port, principal, command), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(0.4)
    child.kill()
    stdout, stderr = child.communicate()
    marker = HOME / "repo-side-effect"
    return {
        "exit_code": child.returncode if child.returncode is not None else -1,
        "stdout": stdout, "stderr": stderr,
        "side_effect": marker.exists(),
    }

CONFIG = """Port %s
ListenAddress 127.0.0.1
HostKey %s
PidFile %s
AuthorizedKeysCommand %s %%f
AuthorizedKeysCommandUser nobody
PermitUserEnvironment yes
UsePAM yes
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PubkeyAuthentication yes
AuthorizedKeysFile none
PermitRootLogin no
StrictModes no
UseDNS no
LogLevel ERROR
"""

AKC = r'''#!/usr/bin/python3
import sys
clients = __CLIENTS__
public_keys = __PUBLIC_KEYS__
fingerprint = sys.argv[-1]
principal = clients.get(fingerprint)
if principal is None:
    sys.exit(1)
sys.stdout.write('environment="MUSEHUB_PRINCIPAL=%s",command="/run/musehub-ssh-forced.py",restrict %s\n' % (principal, public_keys[principal]))
'''

FORCED = r'''#!/usr/bin/python3
import os, re, sys, time
CANCEL = __CANCEL__
ALLOWED = ("git-upload-pack", "git-receive-pack")
LOGICAL = "demo"
UUID_PATH = "repositories/00000000-0000-4000-8000-000000000001/11111111-1111-4111-8111-111111111111.git"
SEGMENT = re.compile(r"^[A-Za-z0-9._-]+$")

def reject(reason):
    sys.stderr.write(reason + "\n")
    sys.exit(1)

principal = os.environ.get("MUSEHUB_PRINCIPAL", "")
if not principal or not SEGMENT.fullmatch(principal):
    reject("missing server principal")
raw = os.environ.get("SSH_ORIGINAL_COMMAND", "")
matched = re.fullmatch(r"(git-upload-pack|git-receive-pack) '([^']*)'", raw) or re.fullmatch(r"(git-upload-pack|git-receive-pack) ([^'\\s]+)", raw)
if matched is None or matched.group(1) not in ALLOWED:
    reject("command not allowed: " + raw)
logical = matched.group(2)
if logical != LOGICAL or not SEGMENT.fullmatch(logical) or logical == ".." or "/" in logical:
    reject("logical path not allowed: " + raw)
parts = [matched.group(1), logical]
if CANCEL:
    time.sleep(3)
marker = os.path.expanduser("~/.cache/musehub-ssh-probe/repo-side-effect")
if CANCEL:
    open(marker, "w").close()
sys.stdout.write("principal=%s command=%s logical=%s path=%s\n" % (principal, parts[0], logical, UUID_PATH))
'''

if __name__ == "__main__":
    main()
"#;

/// Inputs of [`probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeConfig {
    /// Absolute path of the Colima CLI.
    pub colima_bin: PathBuf,
    /// Absolute path of Python 3 inside the VM.
    pub python_bin: String,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            colima_bin: PathBuf::from(COLIMA_BIN),
            python_bin: VM_PYTHON.to_string(),
        }
    }
}

impl ProbeConfig {
    /// The Colima Linux VM defaults.
    pub fn new() -> Self {
        Self::default()
    }
}

/// One forced-command session observed through the temporary OpenSSH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReport {
    /// Exit code of the SSH client.
    pub exit_code: i32,
    /// Standard output. A success is exactly one result line.
    pub stdout: String,
    /// Standard error. A rejection carries the reason here.
    pub stderr: String,
    /// Server-injected principal parsed from a successful line.
    pub principal: Option<String>,
    /// Accepted Git command parsed from a successful line.
    pub command: Option<String>,
    /// Accepted logical repository parsed from a successful line.
    pub logical: Option<String>,
    /// Fixed UUID path parsed from a successful line.
    pub mapped_path: Option<String>,
}

/// Evidence collected by [`probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshReport {
    /// Always [`SCOPE`].
    pub scope: &'static str,
    /// The accepted `git-upload-pack` session.
    pub accepted: SessionReport,
    /// The rejected sessions, in the order they were attempted.
    pub rejected: Vec<SessionReport>,
    /// Two sessions that ran at the same time.
    pub concurrent: Vec<SessionReport>,
    /// The session whose client disconnected before the result line.
    pub cancelled: SessionReport,
    /// Whether the cancelled session created a repository side effect.
    pub cancel_side_effect: bool,
}

/// SSH probe failures. Every variant uses `ssh_probe_failed`.
#[derive(Debug, thiserror::Error)]
pub enum SshProbeError {
    #[error("ssh probe step `{step}` failed: `{command}`: {stderr}")]
    Failed {
        step: &'static str,
        command: String,
        stderr: String,
    },
    #[error("ssh probe step `{step}` could not run `{command}`: {source}")]
    Io {
        step: &'static str,
        command: String,
        #[source]
        source: io::Error,
    },
}

impl ErrorCode for SshProbeError {
    fn code(&self) -> &'static str {
        "ssh_probe_failed"
    }
}

/// Runs the forced-command probe inside the Colima Linux VM.
///
/// A missing Colima binary, a stopped VM, or an `sshd` that does not start
/// fails the probe. Nothing is skipped.
pub fn probe(config: &ProbeConfig) -> Result<SshReport, SshProbeError> {
    ensure_colima(config)?;
    let accepted = call(config, &request("one", "alice", "git-upload-pack demo", None))?;
    let mut rejected = Vec::new();
    for command in [
        "git-upload-pack ../demo",
        "git-upload-pack /abs",
        "git-upload-pack 'demo;id'",
        "bash",
        "",
    ] {
        rejected.push(only(call(
            config,
            &request("one", "alice", command, None),
        )?)?);
    }
    let overridden = only(call(
        config,
        &request("one", "alice", "git-upload-pack demo", Some("mallory")),
    )?)?;
    if overridden.principal.as_deref() == Some("mallory") {
        return Err(failed(
            "identity",
            "client SetEnv overrode the server principal",
        ));
    }
    let concurrent = call(
        config,
        &serde_json::json!({
            "action": "concurrent",
            "principals": ["alice", "bob"],
            "calls": [
                {"principal": "alice", "command": "git-upload-pack demo"},
                {"principal": "bob", "command": "git-receive-pack demo"}
            ]
        }),
    )?;
    let cancelled = call(
        config,
        &request("cancel", "alice", "git-receive-pack demo", None),
    )?;
    Ok(SshReport {
        scope: SCOPE,
        accepted: only(accepted)?,
        rejected,
        concurrent: concurrent
            .get("results")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| failed("concurrent", "missing results"))?
            .iter()
            .map(session_from_json)
            .collect::<Result<Vec<_>, _>>()?,
        cancelled: only(cancelled.clone())?,
        cancel_side_effect: cancelled
            .pointer("/results/0/side_effect")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

fn request(action: &str, principal: &str, command: &str, setenv: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "action": action,
        "principal": principal,
        "principals": [principal],
        "command": command
    });
    if let Some(setenv) = setenv {
        value["setenv"] = serde_json::Value::String(setenv.to_string());
    }
    value
}

fn only(value: serde_json::Value) -> Result<SessionReport, SshProbeError> {
    let result = value
        .pointer("/results/0")
        .ok_or_else(|| failed("session", "the VM harness returned no session"))?;
    session_from_json(result)
}

fn session_from_json(value: &serde_json::Value) -> Result<SessionReport, SshProbeError> {
    let stdout = value
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let stderr = value
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let exit_code = value
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| failed("session", "missing exit code"))? as i32;
    let mut report = SessionReport {
        exit_code,
        stdout: stdout.clone(),
        stderr,
        principal: None,
        command: None,
        logical: None,
        mapped_path: None,
    };
    if let Some(parsed) = parse_success(&stdout) {
        report.principal = Some(parsed.0);
        report.command = Some(parsed.1);
        report.logical = Some(parsed.2);
        report.mapped_path = Some(parsed.3);
    }
    Ok(report)
}

fn parse_success(stdout: &str) -> Option<(String, String, String, String)> {
    let line = stdout.trim();
    let mut fields = line.split(' ');
    let principal = fields.next()?.strip_prefix("principal=")?;
    let command = fields.next()?.strip_prefix("command=")?;
    let logical = fields.next()?.strip_prefix("logical=")?;
    let path = fields.next()?.strip_prefix("path=")?;
    if fields.next().is_some() {
        return None;
    }
    Some((
        principal.to_string(),
        command.to_string(),
        logical.to_string(),
        path.to_string(),
    ))
}

fn call(config: &ProbeConfig, request: &serde_json::Value) -> Result<serde_json::Value, SshProbeError> {
    let line = format!("{} ssh -- {} -", config.colima_bin.display(), config.python_bin);
    let mut child = Command::new(&config.colima_bin)
        .arg("ssh")
        .arg("--")
        .arg(&config.python_bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| SshProbeError::Io {
            step: "colima ssh",
            command: line.clone(),
            source,
        })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| failed("colima ssh", "stdin missing"))?;
        let request_json = serde_json::to_string(request).unwrap_or_default();
        let payload = format!(
            "import os\nos.environ['MUSEHUB_REQUEST']={request_json}\n{VM_HARNESS}\n",
            request_json = serde_json::to_string(&request_json).unwrap_or_default()
        );
        stdin
            .write_all(payload.as_bytes())
            .map_err(|source| io_error("write harness", &line, source))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|source| io_error("colima ssh", &line, source))?;
    if !output.status.success() {
        return Err(SshProbeError::Failed {
            step: "colima ssh",
            command: line,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let response = stdout
        .lines()
        .rev()
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| failed("colima ssh", &format!("no JSON response: {stdout}")))?;
    let value: serde_json::Value = serde_json::from_str(response)
        .map_err(|error| failed("colima ssh", &format!("invalid JSON ({error}): {response}")))?;
    if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let error = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(response);
        return Err(failed("vm harness", error));
    }
    Ok(value)
}

fn ensure_colima(config: &ProbeConfig) -> Result<(), SshProbeError> {
    if config.colima_bin.is_file() {
        return Ok(());
    }
    Err(SshProbeError::Failed {
        step: "resolve colima binary",
        command: config.colima_bin.display().to_string(),
        stderr: format!(
            "{} does not exist; the probe uses absolute paths only",
            config.colima_bin.display()
        ),
    })
}

fn failed(step: &'static str, stderr: &str) -> SshProbeError {
    SshProbeError::Failed {
        step,
        command: format!("{COLIMA_BIN} ssh -- {VM_PYTHON} -"),
        stderr: stderr.to_string(),
    }
}

fn io_error(step: &'static str, command: &str, source: io::Error) -> SshProbeError {
    SshProbeError::Io {
        step,
        command: command.to_string(),
        source,
    }
}

/// Returns whether a client command is one explicit allowed Git command.
pub fn command_allowed(command: &str) -> bool {
    let mut parts = command.split(' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(name), Some(logical), None) => {
            ALLOWED_COMMANDS.contains(&name) && logical_allowed(logical)
        }
        _ => false,
    }
}

fn logical_allowed(logical: &str) -> bool {
    logical == DEMO_LOGICAL
        && !logical.is_empty()
        && logical != ".."
        && !logical.starts_with('/')
        && logical
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_names_the_colima_linux_vm() {
        assert_eq!(SCOPE, "linux_openssh_colima");
        assert!(VM_HARNESS.contains("/usr/sbin/sshd"));
        assert!(VM_HARNESS.contains("PermitUserEnvironment yes"));
        assert!(!VM_HARNESS.contains("git-upload-pack' "));
    }

    #[test]
    fn only_explicit_git_commands_pass_the_allow_list() {
        assert!(command_allowed("git-upload-pack demo"));
        assert!(command_allowed("git-receive-pack demo"));
        for rejected in [
            "",
            "bash",
            "git-upload-pack",
            "git-upload-pack ../demo",
            "git-upload-pack /abs",
            "git-upload-pack demo extra",
            "git-upload-pack 'demo;id'",
            "git-upload-pack demo;id",
        ] {
            assert!(!command_allowed(rejected), "{rejected} must be rejected");
        }
    }

    #[test]
    fn success_line_carries_the_server_principal_and_uuid_path() {
        let (principal, command, logical, path) = parse_success(
            "principal=alice command=git-upload-pack logical=demo path=repositories/00000000-0000-4000-8000-000000000001/11111111-1111-4111-8111-111111111111.git\n",
        )
        .expect("success line");
        assert_eq!(principal, "alice");
        assert_eq!(command, "git-upload-pack");
        assert_eq!(logical, DEMO_LOGICAL);
        assert_eq!(path, DEMO_UUID_PATH);
    }

    #[test]
    fn missing_colima_binary_fails_without_a_process() {
        let config = ProbeConfig {
            colima_bin: PathBuf::from("/nonexistent/colima"),
            ..ProbeConfig::default()
        };
        let error = probe(&config).expect_err("missing binary");
        assert_eq!(error.code(), "ssh_probe_failed");
        assert!(error.to_string().contains("/nonexistent/colima"));
    }
}
