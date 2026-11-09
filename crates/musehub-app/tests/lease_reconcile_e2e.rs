//! Active worker leases must survive orphan reconciliation (multi-instance safety).

mod common;

use musehub_app::reconcile_orphaned_work;

#[tokio::test]
async fn reconcile_skips_runs_with_active_lease() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let _app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;

    let org_id: (uuid::Uuid,) =
        sqlx::query_as("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let repo_id = uuid::Uuid::new_v4();
    let storage_key = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state)
         VALUES ($1,$2,'lease-e2e','lease-e2e',$3,'ready')",
    )
    .bind(repo_id)
    .bind(org_id.0)
    .bind(storage_key)
    .execute(&pool)
    .await
    .unwrap();

    let issue_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, title, state, author_id)
         VALUES ($1,$2,$3,1,'lease','open',
           (SELECT id FROM principals WHERE handle_normalized = 'admin' LIMIT 1))",
    )
    .bind(issue_id)
    .bind(org_id.0)
    .bind(repo_id)
    .execute(&pool)
    .await
    .unwrap();

    let peer = uuid::Uuid::new_v4();
    let agent_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state,
         requested_by, lease_holder, lease_until)
         VALUES ($1,$2,$3,$4,'reference','agent/lease','running',
           (SELECT id FROM principals WHERE handle_normalized = 'admin' LIMIT 1),$5,
           CURRENT_TIMESTAMP + INTERVAL '1 hour')",
    )
    .bind(agent_id)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(issue_id)
    .bind(peer)
    .execute(&pool)
    .await
    .unwrap();

    let report = reconcile_orphaned_work(&pool).await.unwrap();
    assert_eq!(report.agent_runs_failed, 0);

    let state: (String,) = sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "running");

    sqlx::query(
        "UPDATE agent_runs SET lease_until = CURRENT_TIMESTAMP - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(agent_id)
    .execute(&pool)
    .await
    .unwrap();
    let report2 = reconcile_orphaned_work(&pool).await.unwrap();
    assert_eq!(report2.agent_runs_failed, 1);
    let state2: (String, Option<String>) =
        sqlx::query_as("SELECT state, error_code FROM agent_runs WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state2.0, "failed");
    assert_eq!(state2.1.as_deref(), Some("orphaned_after_restart"));
}
