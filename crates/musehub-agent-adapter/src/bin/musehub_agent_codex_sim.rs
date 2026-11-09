//! Simulated Codex CLI protocol (NDJSON on stdout). No network or API keys.

use std::io::{self, BufRead, Write};

use musehub_agent_adapter::{AgentTask, CODEX_ADAPTER};

fn main() {
    if let Err(error) = run() {
        let _ = writeln!(
            io::stderr(),
            "{}",
            serde_json::json!({
                "type": "error",
                "adapter": CODEX_ADAPTER,
                "detail": error.to_string(),
            })
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let line = io::stdin()
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
            "adapter": CODEX_ADAPTER,
            "simulated": true,
        })
    )?;
    writeln!(
        stdout,
        "{}",
        serde_json::json!({
            "type": "usage",
            "tokens": 1,
            "microusd": 0,
            "budget_unknown": true,
        })
    )?;
    writeln!(
        stdout,
        "{}",
        serde_json::json!({
            "type": "completed",
            "adapter": CODEX_ADAPTER,
            "kind": task.kind,
            "commit": null,
            "simulated": true,
            "note": "no live codex credentials; protocol-only probe",
        })
    )?;
    Ok(())
}
