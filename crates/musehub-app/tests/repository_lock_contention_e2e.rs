//! Separate PostgreSQL pools model independent MuseHub instances contending on one repository.

use musehub_app::repository_lock::acquire_repository_write_lock;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn independent_pools_serialize_repository_writes() {
    let database_url = std::env::var("MUSEHUB_TEST_DATABASE_URL")
        .expect("MUSEHUB_TEST_DATABASE_URL must point to an acceptance database");
    let pool_a = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("first instance pool connects");
    let pool_b = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("second instance pool connects");

    let repo_id = Uuid::new_v4();
    let held = acquire_repository_write_lock(&pool_a, repo_id)
        .await
        .expect("first instance acquires repository lock");

    let waiter_pool = pool_b.clone();
    let mut waiter = tokio::spawn(async move {
        let waiting = acquire_repository_write_lock(&waiter_pool, repo_id)
            .await
            .expect("second instance waits for repository lock");
        waiting
            .commit()
            .await
            .expect("second instance releases lock");
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut waiter)
            .await
            .is_err(),
        "a second independent pool must not enter the same repository write section"
    );

    held.commit().await.expect("first instance releases lock");
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("second instance resumes after release")
        .expect("second instance task succeeds");

    pool_b.close().await;
    pool_a.close().await;
}
