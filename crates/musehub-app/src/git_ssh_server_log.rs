//! Git-over-SSH server-side diagnostics (never written to the SSH client stderr stream).

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

const SERVER_LOG_ENV: &str = "MUSEHUB_GIT_SSH_SERVER_LOG";

static LOG_MUTEX: Mutex<()> = Mutex::new(());

/// Append one line to the configured server log file (no-op when unset or unreadable).
pub fn append_server_log(line: &str) {
    let Ok(path) = std::env::var(SERVER_LOG_ENV) else {
        return;
    };
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    let _guard = LOG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

pub fn log_pack_plan_failure(detail: &str) {
    append_server_log(&format!("ssh pack plan failed: {detail}"));
}

pub fn log_pack_exec_failure(detail: &str, service: &str) {
    append_server_log(&format!("git {service} failed: {detail}"));
}
