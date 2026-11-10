//! Compose health mock adapter — validates allowlisted target env, emits NDJSON summary.
//! Does not invoke Docker or mount socket.

fn main() {
    let target =
        std::env::var("MUSEHUB_DEPLOY_HEALTH_TARGET").unwrap_or_else(|_| "mock://healthy".into());
    let manifest = std::env::var("MUSEHUB_RELEASE_MANIFEST_SHA256").unwrap_or_default();
    if target != "mock://healthy"
        && !target.starts_with("http://127.0.0.1:")
        && !target.starts_with("http://localhost:")
    {
        println!(r#"{{"type":"error","code":"health_target_denied"}}"#);
        std::process::exit(1);
    }
    println!(r#"{{"type":"started","target":"{target}"}}"#);
    println!(r#"{{"type":"health","status":"healthy","manifest_sha256":"{manifest}"}}"#);
    println!(r#"{{"type":"completed","outcome":"succeeded"}}"#);
}
