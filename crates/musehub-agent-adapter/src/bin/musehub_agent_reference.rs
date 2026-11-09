//! Deterministic reference adapter: reads one JSON task line on stdin, emits NDJSON on stdout.
//! Makes a real local Git commit in the current working directory when `kind=implementation`.

use std::io::{self, BufRead, Write};
use std::process::Command;

use musehub_agent_adapter::{AgentTask, REFERENCE_ADAPTER};

fn main() {
    if let Err(error) = run() {
        let _ = writeln!(
            io::stderr(),
            "{{\"type\":\"error\",\"adapter\":\"{REFERENCE_ADAPTER}\",\"detail\":{}}}",
            serde_json::to_string(&error.to_string()).unwrap_or_else(|_| "\"error\"".to_string())
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .transpose()?
        .ok_or("missing task line on stdin")?;
    let task: AgentTask = serde_json::from_str(&line)?;
    let mut stdout = io::stdout();
    writeln!(
        stdout,
        "{}",
        serde_json::json!({
            "type": "started",
            "adapter": REFERENCE_ADAPTER,
        })
    )?;
    writeln!(
        stdout,
        "{}",
        serde_json::json!({
            "type": "progress",
            "detail": "reference adapter running",
        })
    )?;
    writeln!(
        stdout,
        "{}",
        serde_json::json!({
            "type": "usage",
            "tokens": 0,
            "microusd": 0,
            "budget_unknown": true,
        })
    )?;
    if task.message == "__slow__" {
        std::thread::sleep(std::time::Duration::from_secs(8));
    }
    let commit = if task.kind == "implementation" {
        let branch = task.branch.as_deref().unwrap_or("agent/reference");
        ensure_branch(branch)?;
        std::fs::write("AGENT_TASK.txt", &task.message)?;
        git(&["add", "--", "AGENT_TASK.txt"])?;
        git(&[
            "-c",
            "user.email=agent@musehub.local",
            "-c",
            "user.name=musehub-agent-reference",
            "commit",
            "--no-gpg-sign",
            "-m",
            &format!("agent(reference): {}", task.message),
        ])?;
        Some(git(&["rev-parse", "HEAD"])?)
    } else {
        None
    };
    writeln!(
        stdout,
        "{}",
        serde_json::json!({
            "type": "completed",
            "adapter": REFERENCE_ADAPTER,
            "kind": task.kind,
            "commit": commit,
        })
    )?;
    Ok(())
}

fn ensure_branch(branch: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = git(&["checkout", "-B", branch]);
    Ok(())
}

fn git(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
