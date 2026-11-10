//! Remote agent: stale lease epoch rejects terminal write from old worker.

mod common;

use musehub_app::lease::{bump_agent_run_epoch_for_test, finalize_agent_run_state};
use uuid::Uuid;

#[tokio::test]
async fn remote_stale_epoch_cannot_finalize_after_bump() {
    std::env::set_var("MUSEHUB_AGENT_EXECUTOR", "remote");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let _app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;

    let org_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let repo_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state)
         VALUES ($1,$2,'agent-epoch','agent-epoch',$3,'ready')",
    )
    .bind(repo_id)
    .bind(org_id.0)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let issue_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, title, state, author_id)
         VALUES ($1,$2,$3,99,'epoch','open',
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1))",
    )
    .bind(issue_id)
    .bind(org_id.0)
    .bind(repo_id)
    .execute(&pool)
    .await
    .unwrap();

    let run_id = Uuid::new_v4();
    let stale_instance = Uuid::new_v4();
    let stale_epoch = 4_i64;
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state,
         requested_by, lease_holder, lease_epoch, lease_until)
         VALUES ($1,$2,$3,$4,'reference','agent/epoch','running',
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1),
           $5, $6, CURRENT_TIMESTAMP + INTERVAL '1 hour')",
    )
    .bind(run_id)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(issue_id)
    .bind(stale_instance)
    .bind(stale_epoch)
    .execute(&pool)
    .await
    .unwrap();

    let new_epoch = bump_agent_run_epoch_for_test(&pool, run_id).await.unwrap();
    assert!(new_epoch > stale_epoch);

    let applied = finalize_agent_run_state(
        &pool,
        run_id,
        stale_instance,
        stale_epoch,
        "succeeded",
        None,
        Some("f".repeat(40).as_str()),
        Some(1),
    )
    .await
    .unwrap();
    assert!(!applied);

    let row: (String,) = sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "running");
    std::env::remove_var("MUSEHUB_AGENT_EXECUTOR");
}
