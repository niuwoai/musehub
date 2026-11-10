//! Controlled external CLI fixture for Codex-shaped NDJSON (not an online model).
//!
//! Modes are selected via `AgentTask.message` prefix after `fixture:`:
//! - `fixture:ok` — immediate completed
//! - `fixture:slow` — sleep 45s (cancel/timeout tests)
//! - `fixture:budget` — usage exceeding typical budget
//! - `fixture:timeout` — never completes (parent deadline)

use std::io::{self, BufRead, Write};
use std::thread;
use std::time::Duration;

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
    let mode = task.message.strip_prefix("fixture:").unwrap_or("ok");
    let mut stdout = io::stdout();
    writeln!(
        stdout,
        "{}",
        serde_json::json!({
            "type": "started",
            "adapter": CODEX_ADAPTER,
            "fixture": true,
            "mode": mode,
        })
    )?;
    match mode {
        "budget" => {
            writeln!(
                stdout,
                "{}",
                serde_json::json!({
                    "type": "usage",
                    "tokens": 50_000,
                    "microusd": 0,
                })
            )?;
            thread::sleep(Duration::from_millis(50));
            writeln!(
                stdout,
                "{}",
                serde_json::json!({
                    "type": "completed",
                    "adapter": CODEX_ADAPTER,
                    "commit": null,
                    "fixture": true,
                })
            )?;
        }
        "slow" => {
            for _ in 0..90 {
                thread::sleep(Duration::from_millis(500));
            }
            writeln!(
                stdout,
                "{}",
                serde_json::json!({
                    "type": "completed",
                    "adapter": CODEX_ADAPTER,
                    "commit": null,
                    "fixture": true,
                    "note": "slow fixture finished without commit",
                })
            )?;
        }
        "timeout" => loop {
            thread::sleep(Duration::from_secs(3600));
        },
        _ => {
            writeln!(
                stdout,
                "{}",
                serde_json::json!({
                    "type": "usage",
                    "tokens": 3,
                    "microusd": 0,
                })
            )?;
            writeln!(
                stdout,
                "{}",
                serde_json::json!({
                    "type": "completed",
                    "adapter": CODEX_ADAPTER,
                    "commit": null,
                    "fixture": true,
                    "note": "fixture ok; not online codex",
                })
            )?;
        }
    }
    Ok(())
}
