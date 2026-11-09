//! Stale worker cannot write terminal state after lease epoch advances.

mod common;

use musehub_app::lease::{bump_agent_run_epoch_for_test, finalize_agent_run_state};
use uuid::Uuid;

#[tokio::test]
async fn stale_epoch_cannot_finalize_succeeded() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
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
         VALUES ($1,$2,'fence','fence',$3,'ready')",
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
         VALUES ($1,$2,$3,1,'f','open',
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
    let stale_epoch = 3_i64;
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state,
         requested_by, lease_holder, lease_epoch, lease_until)
         VALUES ($1,$2,$3,$4,'reference','agent/f','running',
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
    assert_eq!(new_epoch, stale_epoch + 1);

    let applied = finalize_agent_run_state(
        &pool,
        run_id,
        stale_instance,
        stale_epoch,
        "succeeded",
        None,
        Some("c".repeat(40).as_str()),
        Some(1),
    )
    .await
    .unwrap();
    assert!(!applied, "stale worker must not write terminal state");

    let row: (String, Option<String>) =
        sqlx::query_as("SELECT state, result_commit_oid FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "running");
    assert!(row.1.is_none());
}
