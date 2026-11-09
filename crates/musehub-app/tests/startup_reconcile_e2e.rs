//! Server prepare reconciles orphaned queued/running work after restart.

mod common;

use musehub_app::reconcile_orphaned_work;

#[tokio::test]
async fn reconcile_marks_orphan_agent_and_pipeline_runs() {
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
         VALUES ($1,$2,'reconcile-e2e','reconcile-e2e',$3,'ready')",
    )
    .bind(repo_id)
    .bind(org_id.0)
    .bind(storage_key)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1,$2,$3,$4,'maintainer')
         ON CONFLICT (repo_id, principal_id) DO NOTHING",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(org_id.0)
    .bind(repo_id)
    .bind(
        sqlx::query_as::<_, (uuid::Uuid,)>(
            "SELECT id FROM principals WHERE handle_normalized = 'admin'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .0,
    )
    .execute(&pool)
    .await
    .unwrap();
    let issue_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, title, state, author_id)
         VALUES ($1,$2,$3,424242,'reconcile-fixture','open',
           (SELECT id FROM principals WHERE handle_normalized = 'admin' LIMIT 1))",
    )
    .bind(issue_id)
    .bind(org_id.0)
    .bind(repo_id)
    .execute(&pool)
    .await
    .unwrap();
    let admin_id: (uuid::Uuid,) =
        sqlx::query_as("SELECT id FROM principals WHERE handle_normalized = 'admin'")
            .fetch_one(&pool)
            .await
            .unwrap();

    let agent_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state, requested_by)
         VALUES ($1,$2,$3,$4,'reference','agent/reconcile','running',$5)",
    )
    .bind(agent_id)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(issue_id)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();

    let cancel_agent = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, adapter_kind, work_branch, state, requested_by)
         VALUES ($1,$2,$3,$4,'reference','agent/cancel','cancel_requested',$5)",
    )
    .bind(cancel_agent)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(issue_id)
    .bind(admin_id.0)
    .execute(&pool)
    .await
    .unwrap();

    let pipe_id = uuid::Uuid::new_v4();
    let job_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
         VALUES ($1,$2,$3,$4,'product-ci','running',$5,$6,'release_build')",
    )
    .bind(pipe_id)
    .bind(org_id.0)
    .bind(repo_id)
    .bind("a".repeat(40))
    .bind(admin_id.0)
    .bind(uuid::Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,'build','running')",
    )
    .bind(job_id)
    .bind(org_id.0)
    .bind(repo_id)
    .bind(pipe_id)
    .execute(&pool)
    .await
    .unwrap();

    let report = reconcile_orphaned_work(&pool).await.unwrap();
    assert_eq!(report.agent_runs_failed, 1);
    assert_eq!(report.agent_runs_canceled, 1);
    assert_eq!(report.pipeline_runs_failed, 1);

    let agent_state: (String, Option<String>) =
        sqlx::query_as("SELECT state, error_code FROM agent_runs WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(agent_state.0, "failed");
    assert_eq!(agent_state.1.as_deref(), Some("orphaned_after_restart"));

    let cancel_state: (String,) =
        sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
            .bind(cancel_agent)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cancel_state.0, "canceled");

    let job_state: (String,) =
        sqlx::query_as("SELECT state FROM pipeline_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(job_state.0 == "failed" || job_state.0 == "canceled");
}
