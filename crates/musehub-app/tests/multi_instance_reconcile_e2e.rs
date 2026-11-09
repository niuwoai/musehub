//! Two control-plane instance ids: each peer's leased run survives reconcile.

mod common;

use musehub_app::reconcile_orphaned_work;
use uuid::Uuid;

#[tokio::test]
async fn reconcile_respects_two_peer_instance_leases() {
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
    let storage_key = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state)
         VALUES ($1,$2,'multi-inst','multi-inst',$3,'ready')",
    )
    .bind(repo_id)
    .bind(org_id.0)
    .bind(storage_key)
    .execute(&pool)
    .await
    .unwrap();
    let issue_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, title, state, author_id)
         VALUES ($1,$2,$3,1,'t','open',
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1))",
    )
    .bind(issue_id)
    .bind(org_id.0)
    .bind(repo_id)
    .execute(&pool)
    .await
    .unwrap();

    let inst_a = Uuid::new_v4();
    let inst_b = Uuid::new_v4();
    let run_a = Uuid::new_v4();
    let run_b = Uuid::new_v4();
    for (run_id, peer) in [(run_a, inst_a), (run_b, inst_b)] {
        sqlx::query(
            "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state,
             requested_by, lease_holder, lease_until, lease_epoch)
             VALUES ($1,$2,$3,$4,'reference','agent/x','running',
               (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1),
               $5, CURRENT_TIMESTAMP + INTERVAL '1 hour', 2)",
        )
        .bind(run_id)
        .bind(org_id.0)
        .bind(repo_id)
        .bind(issue_id)
        .bind(peer)
        .execute(&pool)
        .await
        .unwrap();
    }

    let report = reconcile_orphaned_work(&pool).await.unwrap();
    assert_eq!(report.agent_runs_failed, 0);
    for id in [run_a, run_b] {
        let state: (String,) = sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state.0, "running");
    }
}
