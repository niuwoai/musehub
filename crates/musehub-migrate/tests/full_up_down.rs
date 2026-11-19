//! Full migration chain on the dedicated CI/acceptance Postgres; requires `MUSEHUB_TEST_DATABASE_URL` and explicit reset authorization.
//!
//! Simulates the acceptance flake: an idle pooled connection during `migrate down`.

use std::path::PathBuf;

use musehub_migrate::{
    business_tables, connect, load_migrations, maybe_terminate_idle_peer_backends, migrate_down,
    migrate_up,
};

fn migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations")
}

fn require_database_url() -> String {
    assert_eq!(
        std::env::var("MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET").as_deref(),
        Ok("1"),
        "set MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1 to authorize the migration up/down reset"
    );
    std::env::var("MUSEHUB_TEST_DATABASE_URL")
        .expect("set MUSEHUB_TEST_DATABASE_URL to the dedicated acceptance database")
}

#[tokio::test]
async fn full_schema_up_down_up_round_trip() {
    let url = require_database_url();
    std::env::set_var("MUSEHUB_MIGRATE_TERMINATE_IDLE_PEERS", "1");
    let pool = connect(&url).await.expect("connect");
    let migrations = load_migrations(&migrations_dir()).expect("load");
    migrate_down(&pool, &migrations)
        .await
        .expect("reset dedicated test database");
    let peer = connect(&url).await.expect("second instance connect");
    assert!(
        migrations.len() >= 27,
        "expected full migration set, got {}",
        migrations.len()
    );

    let (up, peer_up) = tokio::join!(
        migrate_up(&pool, &migrations),
        migrate_up(&peer, &migrations)
    );
    let up = up.expect("first concurrent up");
    let peer_up = peer_up.expect("second concurrent up");
    assert_eq!(
        up.applied().len() + peer_up.applied().len(),
        migrations.len(),
        "concurrent up must apply each migration exactly once"
    );
    assert_eq!(
        up.skipped().len() + peer_up.skipped().len(),
        migrations.len(),
        "each migration must be applied by one instance and observed by the other"
    );

    let down = migrate_down(&pool, &migrations).await.expect("down");
    assert_eq!(
        down.applied().len(),
        migrations.len(),
        "down applied {:?}, expected {} steps",
        down.applied(),
        migrations.len()
    );
    let tables = business_tables(&pool).await.expect("tables");
    assert!(
        tables.is_empty(),
        "business tables left after full down: {tables:?}"
    );

    let again = migrate_up(&pool, &migrations).await.expect("second up");
    assert_eq!(again.applied().len(), migrations.len());
}

#[tokio::test]
async fn down_succeeds_with_idle_peer_pool_connection() {
    let url = require_database_url();
    std::env::set_var("MUSEHUB_MIGRATE_TERMINATE_IDLE_PEERS", "1");
    let pool = connect(&url).await.expect("connect");
    let migrations = load_migrations(&migrations_dir()).expect("load");
    migrate_up(&pool, &migrations).await.expect("up");

    let peer = connect(&url).await.expect("peer connect");
    let hold = peer.acquire().await.expect("hold idle connection");

    let down = migrate_down(&pool, &migrations).await;
    drop(hold);
    down.expect("down with idle peer should succeed when peers are terminated");

    let tables = business_tables(&pool).await.expect("tables");
    assert!(tables.is_empty(), "tables after down: {tables:?}");

    migrate_up(&pool, &migrations)
        .await
        .expect("restore schema");
}

#[tokio::test]
async fn terminate_idle_peers_is_noop_without_env() {
    let url = require_database_url();
    std::env::remove_var("MUSEHUB_MIGRATE_TERMINATE_IDLE_PEERS");
    let pool = connect(&url).await.expect("connect");
    maybe_terminate_idle_peer_backends(&pool)
        .await
        .expect("noop terminate");
}
