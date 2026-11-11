//! Atomic worker claim: duplicate spawns leave one runner; renew past TTL survives reconcile.

mod common;

use musehub_app::agent_supervisor::{new_supervisor, run_agent_job};
use musehub_app::reconcile_orphaned_work;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn duplicate_worker_spawn_only_one_succeeds() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let _app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;

    let org_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let repo_id = Uuid::new_v4();
    let storage_key = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state)
         VALUES ($1,$2,'worker-dup','worker-dup',$3,'ready')",
    )
    .bind(repo_id)
    .bind(org_id.0)
    .bind(storage_key)
    .execute(&pool)
    .await
    .unwrap();
    musehub_app::init_bare_repository(git_root.path(), &storage_key).unwrap();
    let issue_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, title, state, author_id)
         VALUES ($1,$2,$3,1,'dup','open',
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1))",
    )
    .bind(issue_id)
    .bind(org_id.0)
    .bind(repo_id)
    .execute(&pool)
    .await
    .unwrap();

    let run_id = Uuid::new_v4();
    let instance = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, requested_by, adapter_kind, work_branch, state)
         VALUES ($1,$2,$3,$4,
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1),
           'reference','agent/dup','queued')",
    )
    .bind(run_id)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(issue_id)
    .execute(&pool)
    .await
    .unwrap();

    let supervisor = new_supervisor();
    let bin = common::reference_adapter_bin();
    let branch = format!("agent/{run_id}");
    let msg = "dup worker test".to_string();

    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let sup_a = supervisor.clone();
    let sup_b = supervisor.clone();
    let git = git_root.path().to_path_buf();

    let git2 = git.clone();
    let bin2 = bin.clone();
    let branch2 = branch.clone();
    let msg2 = msg.clone();
    let j1 = tokio::spawn(async move {
        run_agent_job(
            pool_a,
            sup_a,
            instance,
            run_id,
            repo_id,
            storage_key,
            git,
            bin,
            branch,
            msg,
            Duration::from_secs(60),
            None,
            None,
        )
        .await;
    });
    let j2 = tokio::spawn(async move {
        run_agent_job(
            pool_b,
            sup_b,
            instance,
            run_id,
            repo_id,
            storage_key,
            git2,
            bin2,
            branch2,
            msg2,
            Duration::from_secs(60),
            None,
            None,
        )
        .await;
    });
    let (_, _) = tokio::join!(j1, j2);

    let row: (String, Option<String>) =
        sqlx::query_as("SELECT state, error_code FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        row.0, "succeeded",
        "winner must complete; error_code={:?}",
        row.1
    );
}

#[tokio::test]
async fn renew_extends_past_ttl_before_reconcile() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let _app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;

    let peer = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let org_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let repo_id = Uuid::new_v4();
    let storage_key = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state)
         VALUES ($1,$2,'worker-renew','worker-renew',$3,'ready')",
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
         VALUES ($1,$2,$3,1,'renew','open',
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1))",
    )
    .bind(issue_id)
    .bind(org_id.0)
    .bind(repo_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state,
         requested_by, lease_holder, lease_until, lease_epoch)
         VALUES ($1,$2,$3,$5,'reference','agent/renew','running',
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1),
           $4, CURRENT_TIMESTAMP - INTERVAL '2 minutes', 3)",
    )
    .bind(run_id)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(peer)
    .bind(issue_id)
    .execute(&pool)
    .await
    .unwrap();

    let report_before = reconcile_orphaned_work(&pool).await.unwrap();
    assert_eq!(report_before.agent_runs_failed, 1);

    let run_id2 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state,
         requested_by, lease_holder, lease_until, lease_epoch)
         VALUES ($1,$2,$3,$5,'reference','agent/renew2','running',
           (SELECT id FROM principals WHERE handle_normalized='admin' LIMIT 1),
           $4, CURRENT_TIMESTAMP - INTERVAL '2 minutes', 5)",
    )
    .bind(run_id2)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(peer)
    .bind(issue_id)
    .execute(&pool)
    .await
    .unwrap();

    musehub_app::lease::renew_agent_work_lease_for_test(&pool, run_id2, peer, 5)
        .await
        .unwrap();

    let report = reconcile_orphaned_work(&pool).await.unwrap();
    assert_eq!(report.agent_runs_failed, 0);
    let state: (String,) = sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id2)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "running");
}
