//! Proves identity migrations on a private PostgreSQL cluster.
//!
//! The cluster lives in a temporary directory and a unix socket. It never uses
//! TCP port 5432 and never connects to a shared or production database.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sqlx::postgres::PgConnectOptions;
use sqlx::postgres::PgPoolOptions;
use sqlx::types::Uuid;
use sqlx::ConnectOptions;
use sqlx::Executor;
use sqlx::Row;

use musehub_migrate::{
    business_tables, check_schema, connect, load_migrations, migrate_down, migrate_up,
};

const POSTGRES_BIN: &str = "/opt/homebrew/opt/postgresql@17/bin";

struct Cluster {
    root: PathBuf,
    socket: PathBuf,
    port: u16,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = Command::new(self.bin("pg_ctl"))
            .args([
                "-D",
                &self.root.join("data").display().to_string(),
                "-m",
                "immediate",
                "stop",
            ])
            .output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Cluster {
    fn start() -> Cluster {
        for name in ["initdb", "postgres", "pg_ctl"] {
            assert!(
                Path::new(POSTGRES_BIN).join(name).is_file(),
                "missing {POSTGRES_BIN}/{name}"
            );
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("musehub-pg-{nanos}"));
        let data = root.join("data");
        let socket = root.join("socket");
        fs::create_dir_all(&socket).expect("socket dir");
        let port = ephemeral_port();
        let init = Command::new(Path::new(POSTGRES_BIN).join("initdb"))
            .args([
                "-D",
                &data.display().to_string(),
                "-U",
                "musehub",
                "--auth=trust",
                "--no-sync",
            ])
            .output()
            .expect("initdb");
        assert!(
            init.status.success(),
            "initdb failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        fs::write(
            data.join("postgresql.conf"),
            format!(
                "listen_addresses = ''\nport = {port}\nunix_socket_directories = '{}'\n",
                socket.display()
            ),
        )
        .expect("postgresql.conf");
        let log = root.join("postgres.log");
        let start = Command::new(Path::new(POSTGRES_BIN).join("pg_ctl"))
            .env("LC_ALL", "C")
            .args([
                "-D",
                &data.display().to_string(),
                "-l",
                &log.display().to_string(),
                "-w",
                "start",
            ])
            .output()
            .expect("pg_ctl");
        assert!(
            start.status.success(),
            "pg_ctl failed: {} {}",
            String::from_utf8_lossy(&start.stderr),
            fs::read_to_string(&log).unwrap_or_default()
        );
        Cluster { root, socket, port }
    }

    fn bin(&self, name: &str) -> PathBuf {
        Path::new(POSTGRES_BIN).join(name)
    }

    fn options(&self, database: &str) -> PgConnectOptions {
        PgConnectOptions::new()
            .socket(&self.socket)
            .port(self.port)
            .username("musehub")
            .database(database)
    }

    async fn admin(&self) -> sqlx::PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_with(self.options("postgres"))
            .await
            .expect("admin connection")
    }
}

fn ephemeral_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    listener.local_addr().expect("local addr").port()
}

fn migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations")
}

#[tokio::test]
async fn identity_migrations_go_up_down_and_up() {
    let cluster = Cluster::start();
    let admin = cluster.admin().await;
    let database = format!("musehub_boot02_{}", std::process::id());
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .expect("create database");

    let url = cluster.options(&database).to_url_lossy().to_string();
    assert!(
        !url.contains(":5432"),
        "isolated cluster must not use the default port: {url}"
    );
    let pool = connect(&url).await.expect("connect");
    let migrations = load_migrations(&migrations_dir()).expect("load");
    assert_eq!(
        migrations
            .iter()
            .map(|item| item.version())
            .collect::<Vec<_>>(),
        vec!["0001", "0002"]
    );

    let first = migrate_up(&pool, &migrations).await.expect("first up");
    assert_eq!(first.applied(), &["0001".to_string(), "0002".to_string()]);
    let check = check_schema(&pool).await.expect("check");
    assert!(
        check.is_ok(),
        "timestamps={:?} indexes={:?}",
        check.missing_timestamps(),
        check.missing_fk_indexes()
    );
    assert_updated_at_changes(&pool).await;

    let down = migrate_down(&pool, &migrations).await.expect("down");
    assert_eq!(down.applied(), &["0002".to_string(), "0001".to_string()]);
    let tables = business_tables(&pool).await.expect("tables");
    assert!(tables.is_empty(), "down left tables: {tables:?}");

    let second = migrate_up(&pool, &migrations).await.expect("second up");
    assert_eq!(second.applied(), &["0001".to_string(), "0002".to_string()]);
    let again = check_schema(&pool).await.expect("second check");
    assert!(again.is_ok());
}

async fn assert_updated_at_changes(pool: &sqlx::PgPool) {
    let id = Uuid::parse_str("00000000-0000-4000-8000-000000000010").expect("uuid");
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'Ada', 'ada', 'Ada', 'active')",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("insert principal");
    let before: String = sqlx::query("SELECT updated_at::text FROM principals WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read timestamp")
        .get(0);
    let started = Instant::now();
    loop {
        sqlx::query("UPDATE principals SET display_name = 'Ada Lovelace' WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .expect("update principal");
        let after: String = sqlx::query("SELECT updated_at::text FROM principals WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("read updated timestamp")
            .get(0);
        if after != before {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "updated_at did not advance"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn missing_postgres_binaries_are_not_silently_skipped() {
    let missing = Path::new(POSTGRES_BIN).join("initdb");
    assert!(missing.is_file());
    let output = Command::new(missing)
        .arg("--version")
        .stderr(Stdio::piped())
        .output()
        .expect("initdb --version");
    assert!(output.status.success());
}
