# PR #7 evidence: web E2E flake root cause (run 42579418386)

## Failure symptom

Playwright `00-product-flow.spec.ts` failed with **element not found** on `data-testid="last-agent-run"` (not a `/succeeded/` text timeout alone). That DOM node only renders when `lastRun` is set in React (`App.tsx`), i.e. after `startAgentRun` returns and polling updates state.

## What CI logs did **not** contain

The GitHub Actions log for job `web` does not capture `musehub-server` stdout or per-run `agent_runs.error_code`. Without server logs, the failure cannot be attributed to a specific `error_code` from CI artifacts alone.

## Empirical local classification (post-fix verification)

| Scenario | `error_code` / outcome |
|----------|-------------------------|
| Second worker on same run (`worker_claim_e2e`) | Loser blocked at `try_begin_agent_work` (`state='queued'` gate); winner `succeeded` |
| Peer instance with active lease (`lease_reconcile_e2e`) | Reconcile skips until `lease_until` expires |
| Expired lease + renew (`worker_claim_e2e::renew_extends_past_ttl_before_reconcile`) | Reconcile skips after `renew_agent_work_lease` |

## Likely cause of first CI failure (963a91f)

Before **enqueue lease + atomic `queued→running` claim** (`0f4f155`+), a race could leave the UI without a completed poll (slow agent on GitHub-hosted runner **or** failed start without surfacing `last-agent-run`). **Run `42580226903`** (push `0f4f155`) exited **success** on `web` with **60s** agent timeout unchanged.

## Not verified in CI

- **`musehub-compose-runner`** real isolated deploy: `probe-compose-runner.sh` only records `docker info` / exit code; default deploy mode remains **mock** unless `MUSEHUB_COMPOSE_ADAPTER_MODE=runner|auto` and Docker is available.
- Digest-pinned image pull on GitHub Actions (no workflow step asserts `compose up` health on a live stack).
