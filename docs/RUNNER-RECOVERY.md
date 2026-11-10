# Remote pipeline runner — recovery & retry semantics (V1)

This documents **observed behavior** in `MUSEHUB_PIPELINE_EXECUTOR=remote` mode. It is **not** cross-machine, zero-dependency production readiness (see `docs/RUNNER-BOUNDARY.md`).

## Claim

- API inserts `pipeline_runs.state = queued` with **no** `lease_holder`.
- Worker `POST /api/v1/runner/pipeline-runs/claim` atomically sets `running`, assigns `lease_holder`, bumps `lease_epoch`.

## Renew (lease TTL)

- Worker must renew before `lease_until` (default **45s**; renew every **15s**).
- Renew requires matching `lease_epoch` (**fencing**). Wrong epoch → **409 Conflict**.
- Renew re-validates PAT (revoked/expired → **401**; scope/repo → **403**).

## Worker loss / TTL expiry

- If `lease_until < now` while `running` or `cancel_requested`, another worker may **`try_reclaim_expired_pipeline`** on the same `run_id` (epoch bumps again).
- `reconcile_orphaned_work` still marks expired **unrenewed** runs `failed` (`orphaned_after_restart`) when no reclaim occurs.

## Terminal state (success / fail / cancel)

- Only the holder with the **current** `lease_epoch` may finalize run/job state (`finalize_pipeline_run_state` / `finalize_pipeline_job_state`).
- Stale workers **cannot** set `succeeded` or ready artifacts after epoch moves (E2E: `runner_recovery_e2e`).

## Cancel

- Session `POST .../pipeline-runs/{id}/cancel` sets `cancel_requested` + `cancel_requested` flag; worker observes `cancel_flag` and finalizes `canceled` when fenced.

## Retry definition (V1)

| Scenario | Retry path |
|----------|------------|
| Lease TTL, peer reclaim | Same `run_id`, new worker, new epoch |
| Reconcile `orphaned_after_restart` | **No** auto-requeue; operator must start a **new** pipeline run |
| Failed adapter | **No** auto-retry in product API |

## Dependencies (honest)

- Runner process: `MUSEHUB_DATABASE_URL`, shared `MUSEHUB_GIT_ROOT` / `MUSEHUB_ARTIFACT_ROOT` with API (same-host subprocess tests in `verify-remote-runner-e2e.sh` **do not** prove cross-host isolation).
