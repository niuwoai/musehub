# Changelog

## [0.20.0-rc7]

### Fixed

- Authorize destructive restore only for the isolated product-stack backup/restore acceptance run.
- Enable BuildKit for the production-stack image build and use the runner image's installed `curl` for readiness checks.
- Resolve current stable Clippy diagnostics in pipeline history and webhook signature test code.

## [0.20.0-rc6]

### Fixed

- Group AgentRun commit publication inputs into a named context to satisfy Clippy without suppressing the lint.

### Added

- Extend the opt-in Runner probe to enqueue a concurrent batch, record peak observed running jobs, and fail unless the requested concurrency is observed alongside the scheduler latency target.

## [0.20.0-rc5]

### Added

- Add an opt-in Runner scheduling latency probe for dedicated repositories with safe manual workflows; report the observed API polling upper bound separately from read-only API load results.

## [0.20.0-rc4]

### Added

- Add an authenticated, fixed-rate, read-only API performance benchmark with dataset preflight and JSON/Markdown evidence reports.

## [0.20.0-rc3]

### Added

- Add browser acceptance for organization webhook subscription creation, one-time secret display, persistence after reload, and disabling.

## [0.20.0-rc2]

### Fixed

- Restore the Issue list scroll position after browser back navigation and reset it when opening the list through a new navigation.

## [0.20.0-rc1]

### Added

- Expand AgentRun diagnostics with frozen Issue and accepted-plan versions, base commit, token budget, localized stage progress, and actionable failure guidance.
- Add Plan, AgentRun, and Release links to Issue delivery timelines.
- Add Release notes and explicit verified pipeline artifact attachments, included in the immutable release manifest.

## [0.19.0-rc2]

### Fixed

- Serialize authenticated MCP HTTP requests so concurrent Bearer tokens cannot cross-contaminate authorization context.

## [0.19.0-rc1]

### Added

- Add organization webhook management, cursor-paginated pipeline history, manual pull request creation, release draft creation and publishing, and desired-versus-observed deployment version details.

### Changed

- Use repository-scoped keyset pagination for pipeline run history.


## [0.18.0-rc41]

### Fixed

- Enforce repository grant roles for SSH writes so reader grants remain fetch-only.
- Align protected-branch SSH coverage with the one-time empty-repository seed import exception and assert tag-test clone success.

## [0.18.0-rc40]

### Fixed

- Fix the SSH acceptance fixture's platform-specific `setgroups` argument type so the Linux security matrix can compile and run.

## [0.18.0-rc39]

### Fixed

- Extend the SSH authorization matrix to verify that a principal with access in one organization cannot clone a repository in another organization.
- Fix the PR check report path to validate the pipeline run's captured pull request base OID.

### Verified

- The cross-organization SSH E2E case is implemented; execution requires a newly created dedicated acceptance database and the Linux SSH fixture environment.

## [0.18.0-rc38]

### Added

- Add a bounded SCIM Bulk subset for ordered Users/Groups create, replace, patch, and delete operations, including references to earlier successful creates, per-operation results, audit reuse, `failOnErrors`, and RFC SCIM response media type.

### Verified

- SCIM unit tests pass 8/8; `scim_bulk_e2e` compiles and covers a Group referencing a User created earlier in the same batch. Database E2E execution remains pending until the dedicated acceptance database is explicitly configured.

## [0.18.0-rc37]

### Fixed

- Restrict SCIM Bulk handler input and output types to crate visibility so the product router compiles without exposing internal transport types.

### Verified

- Added `git_ssh_mirror_import_e2e` coverage for SSH mirror import, protected refs, and later mirror updates; it compiled with `--no-run` in this acceptance cycle. The isolated SSH/Runner script executed the initial mirror import, hidden-ref denial, and protected-`main` denial; the Rust test asserting a later mirror update is rejected has not yet been run.

## [0.18.0-rc36]

### Fixed

- Localize personal access token labels, scope names, repository selection, expiry, empty state, and fallback errors.
- Resolve current workspace Clippy diagnostics in pipeline workflow code, domain helpers, and read-only database error construction.

### Verified

- `iam-members-pat.spec.ts` passes 2/2 against an isolated PostgreSQL 17 database after migrations 0001–0048 down/up; covers one-time token display, no secret echo after reload, selected-repository scope persistence, masked listing, and revocation.
- `config-grants.spec.ts` passes 3/3 against an isolated PostgreSQL 17 database after migrations 0001–0048 down/up, including organization name and default collaborator role persistence.

## [0.18.0-rc35]

### Added

- Extend the isolated backup restore acceptance through rclone replication and materialization before restoring the generated snapshot.

### Fixed

- Handle organization settings load failures and stale responses during organization switches; prevent duplicate saves, localize runtime labels, and show explicit save feedback.

### Verified

- Backup restore acceptance completed the generated snapshot → local rclone copy/check → materialization → restore chain against the dedicated `musehub_backup_restore` database; verified 9 database entity sets, 4 Git refs, 28 Git files, and artifact SHA-256.
- Organization settings browser acceptance now exercises both display-name and default collaborator-role persistence, including the save confirmation.

## [0.18.0-rc34]

### Added

- Run the PILOT-01 report contract unit tests in the portable CI job so catalog validation, evidence qualification, aggregation, and no-overwrite guarantees stay covered.

### Verified

- `python3 scripts/test_pilot_report.py`: 11 tests passed locally.

## [0.18.0-rc33]

### Fixed

- Update the remote pipeline Runner acceptance fixture to use the supported `release_build` path; run the shared-database integration slice serially to avoid test races.

### Verified

- T2–T10 targeted MuseHub integration slice passed: 13 test binaries, 22 tests passed, 0 failed, on an isolated PostgreSQL 17.7 container.

## [0.18.0-rc32]

### Fixed

- Prevent delayed Issue metadata loads from overwriting unsaved label and assignee edits; verify the assignee value in both the PATCH request and response.

### Verified

- Full isolated Playwright suite passed: 60 passed, 1 skipped, 0 failed. PostgreSQL migrations 0001–0048 completed down/up with `schema_check=ok`; `yarn --cwd web typecheck` passed.

## [0.18.0-rc31]

### Added

- Show checks for the current pull request head, report validity, current approval, and check-gate status in the PR detail API and console.

### Fixed

- Resolve the isolated browser acceptance root and temporary directories before creating Git fixtures on macOS, avoiding symlink ancestor rejection.

### Verified

- `pr_gates_e2e` passed 2/2 against a dedicated PostgreSQL container with serialized tests; the PR checks browser flow passed within the full 61-test Playwright run. Full browser suite: 57 passed, 1 skipped, 3 failed (one long Agent start timeout and two Issue assignee cases).
- The isolated browser script migrated PostgreSQL through 0048 down/up with `schema_check=ok`. The independent `scripts/verify-ssh-runner-workflow-e2e.sh` recheck passed: Runner-to-DinD Docker API mTLS, workflow/artifact/cache verification, and a fresh-database migration 0001–0048 up/down/up round trip with `schema_check=ok` after both up runs. Full Playwright remains 57 passed, 1 skipped, 3 failed as listed above.

## [0.18.0-rc30]

### Fixed

- Make Release creation idempotent per repository/version, reject conflicting retries and duplicate tag names, and publish a fixed Git tag with compare-and-swap semantics before marking the Release published.

### Verified

- Isolated PostgreSQL migration up/down/up suite passed (3/3); Release REST create/replay/conflict/tag/publish E2E passed (1/1), and MCP Release publication E2E passed (1/1).

## [0.18.0-rc29]

### Fixed

- Copy acceptance criteria into edited Issue revisions within the same database transaction, and verify MCP AgentRun snapshots and stale-plan rejection through the production MCP path.
- Validate Issue resolution reasons consistently in REST and MCP; reject whitespace-only reasons and reasons supplied for non-terminal states instead of silently discarding them.

### Verified

- Run the production MCP contract E2E suite against a dedicated PostgreSQL container; all 6 tests passed, including snapshot fields, idempotent replay, rejection after Issue revision update, and PR merge gates. The Issue resolution-reason unit test passed (1/1).

## [0.18.0-rc28]

### Added

- Add self-service OIDC identity linking from account security, scoped to the currently authenticated principal and protected by one-time state, browser binding, and conflict checks.
- Add a PILOT-01 results-template command that initializes every task as `not_tested` and leaves metadata and human release decisions blank until real evidence is recorded.

### Fixed

- Let local browser E2E reuse installed web dependencies and system Chrome, resolve macOS temporary paths before creating isolated Git roots, and avoid applying Linux storage-group defaults on macOS.
- Fetch and verify the T1 fix commit from the isolated remote before checking out its changed file, so the exact-baseline Runner acceptance has the commit objects it needs.

### Verified

- Run `repo-code-console.spec.ts` against a dedicated temporary PostgreSQL database; migration down/up through 0046 and the browser test passed (1/1).
- Run `oidc-identity-link.spec.ts` against fixed Keycloak 26.4.2 and a dedicated PostgreSQL 17 database; migrations 0001–0047 passed up/down/up with schema check, and the authorization-code/PKCE self-link browser flow passed (1/1).
- Re-run the opt-in isolated T1 Runner acceptance after correcting the fix branch ancestry; the baseline failed all three tests, the PR head passed all three, DinD mTLS connectivity and artifact/cache checks passed, and migrations 0001–0046 completed up/down/up on a fresh database.
- Mirror and verify the exact T1 baseline and fix OIDs in dedicated Forgejo and OneDev projects, then create the shared Issue and open PR in each; product Runner/Build execution remains pending.

## [0.18.0-rc26]

### Fixed

- Import the deterministic T1 baseline through SSH, keep the exact common base on protected `main`, and place the workflow definition on a separate baseline branch so the acceptance does not bypass protected-branch rules.

## [0.18.0-rc25]

### Fixed

- Support custom Playwright web ports in browser-session API origins and avoid interactive sudo prompts during local E2E setup.
- Import the deterministic T1 baseline commit into the isolated MuseHub acceptance repository and assert exact OID equality before creating the Runner workflow, so other products can use the identical Git starting commit.

### Verified

- Record the 2026-11-19 isolated Runner T1, mTLS, artifact/cache, and migrations 0001–0046 acceptance evidence.

## [0.18.0-rc24]

### Fixed

- Keep the T1 Runner acceptance branches based on the repository's seeded default branch so the baseline workflow push is a fast-forward; verify the product Runner baseline (3 expected failures), repaired PR (3 passing tests), DinD mTLS connectivity, artifact/cache behavior, and migration 0001–0046 up/down/up in the isolated SSH/Runner acceptance.

## [0.18.0-rc23]

### Fixed

- Stop browser E2E when migration rollback fails; require dedicated-database/reset opt-in for config grants and migration integration tests, and refuse occupied service ports.

## [0.18.0-rc22]

### Added

- Include a validated human defect inventory and explicit human release decision in PILOT-01 JSON and Markdown reports.

### Fixed

- Authorize database reset for the isolated browser E2E database in CI.

## [0.18.0-rc21]

### Fixed

- Allow the isolated SSH/Runner acceptance build to pass an optional HTTP(S) proxy to BuildKit without requiring proxy configuration in CI.

## [0.18.0-rc20]

### Fixed

- Run the tenant-scoped webhook delivery and Issue REST console integration suites in product acceptance.

## [0.18.0-rc19]

### Added

- Add an opt-in isolated MuseHub Runner T1 pagination acceptance mode that requires a SHA-256-pinned Node image and checks the failing baseline, Issue/PR head, Runner test logs and PR check state.

## [0.18.0-rc18]

### Fixed

- Preserve keyboard focus after accepting an Issue plan by moving focus to the newly enabled AgentRun action; cover Enter activation and focus handoff in the Playwright acceptance flow.

## [0.18.0-rc17]

### Added

- Add repository-authorized assignee support to MCP Issue create, update, list and get responses; include assignment inputs in write idempotency keys and preserve explicit null clearing.
- Add MCP Issue labels, five workflow states, resolution reasons, and filter-bound cursor pagination.
- Add isolated HTTPS webhook delivery coverage that verifies event payloads, HMAC signatures and retry backoff.

### Fixed

- Share Issue resolution-reason validation across REST and MCP, reject reasons on non-terminal states and whitespace-only reasons, and persist normalized reason text.

## [0.18.0-rc16]

### Added

- Add a CI gate that checks the application version across Cargo manifests and lockfile, the Web package, README, architecture and product overview, and changelog.

## [0.18.0-rc15]

### Fixed

- Read staged workflow artifacts by their declared artifact names while preserving the workspace source paths used during collection.

## [0.18.0-rc14]

### Fixed

- Stage workflow artifacts at their declared workspace paths so the control plane can validate and persist them when artifact names differ from paths.

## [0.18.0-rc13]

### Added

- Extend the isolated SSH Runner acceptance to publish and retrieve workflow artifacts, verify their SHA-256 digests, and prove cache restoration through a dependent job.

## [0.18.0-rc12]

### Fixed

- Run the isolated nested Docker daemon in the dedicated Colima VM's cgroup namespace for cgroup v2 job containers, and mount only client certificates into Runner so it cannot read the CA private key.

## [0.18.0-rc11]

### Fixed

- Configure the isolated nested Docker daemon to use the cgroupfs driver for cgroup v2 hosts.

## [0.18.0-rc10]

### Fixed

- Mount the DinD TLS certificate tree as one isolated volume so the daemon and Runner share the generated CA and client credentials.

## [0.18.0-rc9]

### Fixed

- Pin the E2E Docker context to its exact Colima socket, use mutual TLS for the isolated DinD API, and run migration down/up against a fresh fixture database so workflow records do not invalidate the schema rollback.

## [0.18.0-rc8]

### Fixed

- Seed the isolated DinD daemon through a dedicated local registry so digest-pinned workflow jobs run without external registry access.

## [0.18.0-rc7]

### Fixed

- Pull the digest-pinned workflow image directly into the isolated job daemon so its local image store retains the digest reference used by the Runner.

## [0.18.0-rc6]

### Fixed

- Pass the configured `DOCKER_HOST` through the isolated Runner child-process environment so workflow jobs use the nested Docker daemon.

## [0.18.0-rc5]

### Fixed

- Align Runner workflow JSON canonicalization with the control plane so default-empty artifacts and absent optional reports/cache fields no longer cause SHA-256 mismatches.

## [0.18.0-rc4]

### Fixed

- Extend SSH/Runner E2E failure diagnostics with the parent pipeline executor result and logs so failures before workflow job reporting are visible.

## [0.18.0-rc3]

### Fixed

- Reuse the exact cached digest-pinned workflow image in the isolated SSH/Runner E2E, avoiding an unnecessary registry pull when the profile already has the image.

## [0.18.0-rc2]

### Fixed

- Make SSH/Runner E2E wait on the Runner container state and workflow result instead of depending on a log line being visible through Compose.

## [0.18.0-rc1]

### Added

- Add tenant-scoped audit-driven webhook subscriptions with encrypted signing secrets, durable delivery retries, and public-address-only HTTPS targets.

## [0.17.0-rc2]

## [0.17.0-rc2]

### Fixed

- Restrict backup restore acceptance to a dedicated disposable database and isolated temporary storage; verify all Git refs and restored access, release, artifact, pipeline, and audit records. Disable macOS AppleDouble metadata in Git/artifact tarballs so strict archive validation accepts portable backups.

## [0.17.0-rc1]

### Added

- Support one-time `git push --mirror` import into a newly initialized repository while preserving protected default-branch behavior and hiding platform-internal refs from Git clients.

## [0.16.0-rc3]

### Fixed

- Add recoverable loading, diff, and approval states to the pull request detail page, with localized feedback and stale-response protection.

## [0.16.0-rc2]

### Fixed

- Make the isolated SSH-to-Runner acceptance validate its Colima daemon, fail fast on Runner startup and unexpected workflow terminal states, and capture SSH key fingerprint and service diagnostics.

## [0.16.0-rc1]

### Added

- Persist workflow job artifacts and JUnit summaries, expose authorized downloads in the delivery console, and add bounded trust-isolated dependency caching.

## [0.15.0-rc9]

### Fixed

- Allow the isolated SSH-to-Runner acceptance to use a fresh suffixed Colima context while rejecting general Docker contexts; retain the 10 GiB workspace disk preflight.

## [0.15.0-rc8]

### Fixed

- Make the isolated SSH-to-Runner acceptance require its dedicated Docker context and at least 10 GiB of host workspace disk before building.

## [0.15.0-rc7]

### Added

- PR discussion lists now show validated inline comment locations, including file path and base/head line, in English and Chinese.

## [0.15.0-rc6]

### Fixed

- Print workflow job and attempt error codes, persisted logs, and Runner logs before isolated SSH-to-Runner acceptance cleanup.

## [0.15.0-rc5]

### Fixed

- Preload the digest-pinned workflow image into the isolated nested Docker daemon before starting the Runner job.

## [0.15.0-rc4]

### Fixed

- Qualify the workflow attempt state query in the isolated SSH-to-Runner acceptance to avoid ambiguity between joined tables.

## [0.15.0-rc3]

### Fixed

- Correct the isolated SSH-to-Runner workflow fixture to use the product's sequence-based job schema.

## [0.15.0-rc2]

### Fixed

- Add the required identity and timestamp columns to workflow job dependencies so fresh database migrations pass the schema checks.
- Scope SSH-to-Runner acceptance cleanup to resources carrying a per-run ownership label, and require explicit opt-in before the product acceptance script performs a full migration reset.

## [0.15.0-rc1]

### Added

- Retry failed workflow jobs by resuming all failed or skipped DAG nodes with fresh attempts while preserving successful nodes and attempt-scoped logs.

## [0.14.0-rc31]

### Fixed

- Periodically reconcile ready repository heads and tags into the Git ref trigger outbox so accepted SSH pushes can recover missing workflow events after a process crash.

## [0.14.0-rc30]

### Fixed

- Reconcile workflow job and attempt states when a pipeline runner is orphaned: unfinished attempts become `lost`, and canceled runs close unfinished workflow jobs and attempts as canceled.

## [0.14.0-rc29]

### Added

- Persist isolated workflow job output under each workflow job attempt and expose the attempt ID through the workflow jobs API.

## [0.14.0-rc28]

### Fixed

- Allow digest-pinned workflows triggered only by push, tag, or manual events to run in the isolated job adapter; event selection remains enforced by the control plane.

## [0.14.0-rc27]

### Added

- Persist SSH receive-pack branch and tag creation events in a PostgreSQL outbox and dispatch workflow triggers from the control plane; newly created tags are accepted while tag updates and deletions remain blocked.

## [0.14.0-rc26]

### Added

- Add a versioned workflow definition parser with strict fields, digest-pinned job images, DAG validation, and a stable SHA-256 snapshot hash. Runtime scheduling and isolated job execution remain in progress.

## [0.14.0-rc25]

### Added

- Add isolated PostgreSQL tests for GitHub OAuth state and browser binding, one-time attempt consumption, and stable account mapping without login-based linking.

## [0.14.0-rc24]

### Fixed

- Keep GitHub login provider mocks aligned with the login page and preserve repository history request invalidation without an unused local.

## [0.14.0-rc23]

### Added

- Add GitHub OAuth App login with state and browser binding, stable GitHub account ID mapping, and account provisioning without email-based linking.
- Defer real enterprise IdP interoperability from the V1 acceptance scope; keep GitHub OAuth App as the minimum external login requirement.

## [0.14.0-rc22]

### Fixed

- Delimit the expected Live Codex proof line so sentence punctuation cannot become part of the committed file.

## [0.14.0-rc21]

### Fixed

- Preserve the temporary AgentRun repository after Live Codex acceptance failures so committed output mismatches can be diagnosed.

## [0.14.0-rc20]

### Fixed

- Grant Live Codex write access only to the AgentRun's resolved Git metadata directory so workspace-sandboxed runs can create commits.

## [0.14.0-rc19]

### Fixed

- Narrow the Live Codex proof task to the single required file and immediate local commit steps, avoiding unrelated repository exploration.

## [0.14.0-rc18]

### Fixed

- Avoid Codex automatic approval review for isolated AgentRuns by setting `approval_policy=never` while explicitly retaining the `workspace-write` sandbox, reducing extra reviewer model calls without bypassing the sandbox.

## [0.14.0-rc17]

### Fixed

- Run Codex AgentRuns with `--ignore-user-config` so isolated tasks do not inherit ambient Codex user configuration; authentication still uses `CODEX_HOME`.

## [0.14.0-rc16]

### Fixed

- Remove the explicit Codex CLI `--sandbox` argument when using `--approve-for-me`, which already selects the `workspace-write` sandbox and conflicts with that option.

## [0.14.0-rc15]

### Fixed

- Prioritize Codex sandbox and trust diagnostics over generic option requirements, and distinguish unmet option requirements from explicit option conflicts.

## [0.14.0-rc14]

### Fixed

- Classify Codex JSONL error events as well as bounded stderr in memory, and log only fixed categories with the exit status to diagnose early failures without persisting raw stderr or conversation output.

## [0.14.0-rc13]

### Fixed

- Classify bounded Codex stderr in memory and log only a fixed category with the exit status, so early CLI failures can be diagnosed without persisting raw stderr or conversation output.

## [0.14.0-rc12]

### Fixed

- Use Codex CLI's supported `--approve-for-me` option with the `workspace-write` sandbox for unattended AgentRuns; record that the prior unsupported option exited before invoking the model.

## [0.14.0-rc11]

### Fixed

- Attempt unattended Codex execution with approval prompts disabled while retaining the `workspace-write` sandbox; subsequent validation found this flag is unsupported by the installed CLI.

## [0.14.0-rc10]

### Changed

- Raise the opt-in Live Codex acceptance token-budget ceiling from 100,000 to 128,000 after a real run reported 106,172 tokens and was stopped before commit detection.

## [0.14.0-rc9]

### Fixed

- Configure a default Git author identity in temporary AgentRun worktrees when no identity is already available, so Codex can create commits without changing user or global Git configuration.
- Record the sixth Live Codex attempt (96,800 tokens, no commit) and seventh attempt (106,172 tokens, over budget); the seventh run did not validate the temporary Git identity fallback.

## [0.14.0-rc8]

### Fixed

- Make default macOS Git storage owner-private by removing extended ACLs and group/world permissions from the storage root and existing bare repositories, and setting private Git repository modes for subsequent writes.
- Classify Codex failures as unreadable HEAD, uncommitted worktree changes, clean worktree, or unavailable Git status without recording file paths; log only fixed adapter details. Make live acceptance request a temporary Git identity and verify the commit created by Codex.

## [0.14.0-rc7]

### Fixed

- Allow local macOS runs without an explicitly configured shared Git group to create owner-private repositories; explicitly configured shared groups and Linux storage ACLs remain fail-closed.
- Let the Live Codex acceptance select a bounded reasoning effort, defaulting to `low`, to reduce unnecessary model work while retaining provider-reported usage checks.

## [0.14.0-rc6]

### Added

- Add organization-scoped SCIM Group provisioning with membership create/read/list/replace/PATCH/delete, allowlisted filtering and sorting, and audit events. SCIM groups are directory data and do not grant MuseHub permissions.

## [0.14.0-rc5]

### Added

- Persist the minimal provider-reported Codex `turn.completed` usage object on AgentRun terminal states and have the opt-in live acceptance compare its token total with `usage_tokens`; prompts and conversation output are not stored. Live model success remains unverified.

## [0.14.0-rc4]

### Added

- Support allowlisted SCIM User sorting by userName, externalId, displayName, active, and metadata timestamps, with validated direction and deterministic tie-breaking; advertise sorting in ServiceProviderConfig.

## [0.14.0-rc3]

### Fixed

- Preserve RFC 7644 SCIM error responses for router fallback, unsupported media types, and concurrent userName/externalId uniqueness conflicts.
- Keep successful responses and non-SCIM errors unchanged while formatting unmatched SCIM paths as SCIM 404 responses.

## [0.14.0-rc2]

### Fixed

- Recheck the final drained AgentRun usage snapshot against configured budgets after the child process exits, closing a race that could miss an over-budget usage event.
- Record the latest live Codex run's reported usage (140,516 tokens) and clarify that missing raw JSONL prevents independent verification; the 4,096-token threshold remains post-reporting, not a hard per-turn cap.
- Format malformed SCIM JSON, query, and path extractor responses with the RFC 7644 Error schema and `application/scim+json` media type.

## [0.14.0-rc1]

### Added

- Add paginated commit history, directory breadcrumbs, and parent-commit diff browsing to the repository code browser, keeping comparisons pinned to the selected commit; `repo-code-console` passes on local PostgreSQL 17 and covers 20-item pagination, non-duplicated continuation, the final page, text diffs, and paths containing spaces.
- Fix local browser acceptance response ordering and macOS development Git group setup; Git HTTP group switching now uses Linux `sg` only where available.
- Freeze the current Issue revision, acceptance criteria, and accepted plan into AgentRun task input for both REST and MCP starts.
- Let users edit and accept an implementation plan in the Issue page; AgentRun start stays disabled until the current plan is accepted. Add REST regression coverage that verifies AgentRun task input freezes the current Issue revision, acceptance criteria, and accepted plan, plus a passing local Playwright run covering plan acceptance, AgentRun disablement after editing, and re-acceptance.
- Add offline Codex usage parser regressions for cumulative turn snapshots and the input/output token fallback; these validate parsing behavior without claiming a live-model run.

## [0.13.0-rc12]

### Added

- Add a dedicated systemd installer for the asynchronous storage mirror. It validates the root-owned config and service account, installs the service/timer, and enables the timer only when explicitly requested for the primary.

## [0.13.0-rc11]

### Fixed

- Treat Codex `turn.completed.usage` as a cumulative session snapshot instead of summing snapshots across turns; token budget checks use the latest reported total.
- Clarify that token budgets are checked when usage is reported and may not prevent a single model turn from exceeding the configured threshold.

## [0.13.0-rc10]

### Fixed

- Preserve adapter usage totals on failed AgentRuns, including budget-limit failures, so failed runs report measured tokens instead of a misleading zero.

## [0.13.0-rc9]

### Fixed

- Live Codex acceptance now reports API failure context, HTTP status, and response bodies; its generated macOS temporary Git root is canonicalized so the Git SSH symlink guard does not reject `/var` aliases.

## [0.13.0-rc8]

### Fixed

- Corrected PostgreSQL foreign-key index prefix validation for zero-based `int2vector` subscripts and added required `updated_at` columns and triggers to OIDC/SAML login state tables; full PostgreSQL 17.7 migration up/down/up regression passed (3 tests).

## [0.13.0-rc7]

### Fixed

- Restricted receive-pack privilege setup to Linux; unsupported Unix targets now fail closed while the Rust workspace remains portable.
- Grouped AgentRun commit publication inputs into a typed context value to satisfy Clippy's argument-count lint.
- Confirmed independent PostgreSQL repository writers serialize in `repository_lock_contention_e2e` against a disposable PostgreSQL 17 instance.

### Added

- Added a PostgreSQL recovery barrier that binds the quiesced final Git/artifact mirror status to a durable barrier UUID and WAL LSN; the DR verifier checks standby replay and promoted-primary state.
- Extended the isolated Docker acceptance to exercise PostgreSQL streaming replication, lag rejection, standby promotion, barrier verification, and the migration down path.
- Extended `verify-systemd-units.sh` to statically validate the mirror service and timer configuration, including its network dependency, protected environment file, state directory, and one-minute schedule.

## [0.13.0-rc6]

### Fixed

- Continuous storage mirroring now checks file contents, avoiding missed same-size updates with unchanged or coarse-resolution timestamps.

### Added

- Added an isolated Docker acceptance for mirror updates, deletion protection, checksum verification, explicit final-sync acknowledgements, checkpoint preservation after failed passes, and SSH/rsync transport.

## [0.13.0-rc5]

### Added

- Added one-way asynchronous Git/artifact rsync replication over SSH, a systemd timer, success checkpoints, checksum verification, and a quiesced final convergence mode with explicit deletion acknowledgement.

## [0.13.0-rc4]

### Fixed

- Corrected the CloudNativePG DR cluster to use distributed replica topology and removed the unused region-A object-store target.

## [0.13.0-rc3]

### Added

- Added a CloudNativePG Barman Cloud multi-region replica reference, a demotion-token-gated promotion helper that requires verified old-region fencing, and a two-host shared-storage survivor check for post-host-loss write and lock availability.

## [0.13.0-rc2]

### Added

- Added a safe remote backup materializer that downloads to a temporary sibling directory, verifies downloaded bytes against the remote snapshot, validates manifest entries and archive paths, and only then exposes the new local directory. Real object-storage restore remains unverified.

### Fixed

- SCIM user endpoints now return RFC 7644 Error responses with `application/scim+json`; PATCH now supports path-qualified `remove` for `active` and `displayName`.

## [0.13.0-rc1]

### Added

- Added immutable off-host replication for completed PostgreSQL/Git/artifact backup snapshots through rclone, followed by byte-level remote verification. This is point-in-time backup recovery, not live shared storage, synchronous replication, or automatic failover; a write-quiescence window is required for cross-component consistency. Real object-storage replication and restore remain unverified.
- Validated the CloudNativePG HA manifest and synchronous replication in a temporary local k3d cluster. Deleting the primary Pod triggered automatic promotion; the cluster returned to 3/3 Ready and the acknowledged probe row remained readable on the new primary. A second test stopped the k3d worker container hosting the primary; `musehub-postgres-2` was promoted, the cluster recovered to 2/3 Ready, and the acknowledged row remained. Both are single-host Docker/k3d simulations, not evidence of physical-node/network fencing or production failover.
- Exercised rclone snapshot upload and download comparison against a temporary local S3-compatible SeaweedFS service; all four snapshot files matched. An isolated PostgreSQL 17.7 backup→restore also matched a database row, Git main OID, and artifact SHA. Downloading the remote prefix for restoration returned `directory not found`; no real cloud bucket or remote restore was verified.

## [0.12.0-rc1]

### Added

- Added a CloudNativePG three-instance Kubernetes cluster reference with synchronous replication, failover quorum, zone anti-affinity, and a writer LoadBalancer service. Deployment requires an existing HA Kubernetes control plane, storage class, and private LoadBalancer; failover and fencing have not been exercised in this environment.

## [0.11.0-rc2]

### Added

- Strengthened the post-failover verifier: it checks the former primary is no longer writable and validates every configured API instance reports the same version and a writable-primary readiness state.

## [0.11.0-rc1]

### Added

- Extended `verify-shared-storage.sh` with a two-host `flock` acceptance that checks advisory-lock exclusion on both Git and artifact mounts. It still does not prove replication or failover.

## [0.10.0-rc1]

### Added

- Added an external PostgreSQL HA endpoint Compose overlay, startup refusal on standby, primary/standby/read-only readiness reporting, and fail-closed HTTP and shared Git/ref mutation gating when the database is not writable. Added a post-failover endpoint/API verification script; HA cluster orchestration and old-primary fencing remain external and unverified.
- Added an opt-in shared Git/artifact filesystem Compose overlay and a two-host handshake acceptance script for cross-host visibility and atomic rename checks. These checks do not establish storage replication or multi-region HA.
- Added experimental SP-initiated SAML SSO with signed IdP metadata pinning, HTTPS ACS validation, single-use database-backed request correlation, durable replay protection, and administrator-bound `(issuer, NameID)` identities. Real enterprise IdP interoperability remains unverified; the pre-1.0 SAML dependency does not declare an external security audit.
- Added `repository_lock_contention_e2e`, which exercises repository advisory lock contention through independent PostgreSQL pools; it is included in product acceptance but has not yet been run.
- Serialized complete migration up/down operations across service instances with a PostgreSQL advisory lock; the full migration acceptance now races two independent pools and verifies every migration is applied exactly once.

## [0.7.0-rc1]

### Added

- Added organization-scoped SCIM 2.0 user provisioning with expiring, revocable bearer credentials, per-organization external identity mapping, membership lifecycle operations, and audit records. The supported subset is documented; full SCIM interoperability remains outstanding.

## [0.6.0-rc1]

### Added

- Added PostgreSQL transaction-scoped repository write locks across HTTP/SSH receive-pack, PR merge, and AgentRun commit publication to coordinate MuseHub writers across processes sharing the same database.

## [0.5.0-rc1]

### Added

- Added an opt-in standalone-host Runner Compose template and environment example for deployment planning. It remains experimental: cross-host Git locking and long-running physical-host acceptance are not implemented.

## [0.4.0-rc1]

### Added

- Added `scripts/verify-public-tls.sh` to check a deployed gateway's DNS resolution, HTTP-to-HTTPS redirect, trusted hostname certificate, readiness endpoint, and matching version header/body.

## [0.3.0-rc1]

### Added

- Single-provider OIDC Authorization Code + PKCE sign-in with one-time state, browser binding, nonce, validated ID tokens, secure session cookies, and administrator-managed `(issuer, subject)` identity mappings. OIDC users receive no organization membership unless separately granted in MuseHub.

## [0.2.0-rc1]

### Added

- Organization listing, creation with transactional owner membership and audit event, and workspace organization switching.

### Fixed

- Live remote pipeline cancellation acceptance now waits for the slow process to start and checks process exit, canceled terminal state, and artifact quarantine.

## [0.1.0-rc15]

### Added

- Remote Runner acceptance now cancels a running pipeline adapter and verifies process termination, canceled run/job states, and artifact quarantine.

## [0.1.0-rc14]

### Fixed

- The production MCP Python client matrix now fails when its SDK dependencies are missing, validates expected checks, and correctly rejects an unexpectedly accepted unknown tool.

## [0.1.0-rc13]

### Added

- Added paginated repository commit history to the production MCP tool catalog, with cursors bound to the authorized principal, repository, commit, and page size.

## [0.1.0-rc12]

### Fixed

- Production installation and Runner PAT updates now enforce mode `0600` for `deploy/.env`, reject symlink paths, and use a private same-directory temporary file for atomic PAT replacement.

## [0.1.0-rc11]

### Fixed

- AgentRun cancellation now serializes against commit publication; accepted cancellation cannot be overwritten by a later successful push, and requests arriving after publication receive a conflict.

## [0.1.0-rc10]

### Fixed

- Live Codex acceptance now reports unconfirmed AgentRun cancellation after timeout and preserves the temporary workspace for investigation.

## [0.1.0-rc9]

### Fixed

- Codex fixture integration tests now fail when the required fixture binary is missing, rather than silently passing without exercising the checks.

## [0.1.0-rc8]

### Fixed

- Web app version assertion now follows the configured application version instead of stale `0.1.0-rc4` text.

## [0.1.0-rc7]

### Added

- `scripts/verify-live-codex-agent-e2e.sh`: explicitly opt-in real-Codex acceptance with a caller-supplied token budget; it is not part of CI.

### Fixed

- Live Codex adapter invokes `codex exec --json` with the task prompt over stdin, isolates the child environment from Runner secrets, enforces reported token budgets between turns, and requires a new assigned-branch commit for Git-backed AgentRuns; non-Git task workspaces can complete without a commit. Codex monetary usage is unavailable, so configured monetary budgets fail closed.
- Live Codex acceptance revokes its temporary Runner PAT before reporting success, exits unsuccessfully if revocation fails, and cleans the temporary repository even when initialization fails.
- Migration `0005_fk_indexes` for BOOT-02 schema check; Git HTTP route fix (`/git/{org}/{repo}.git/...`).
- Bootstrap 强密码策略；Agent 分支 `agent/*` 校验；Git 路径穿越拒绝。
- `verify-product-acceptance.sh`；CI 强 bootstrap 密码与 reference 二进制预构建。
- Web：`deployment-history-console` 等部署列表切换到「全部历史」与 `deployment-rollback-console` 共用 `switchDeploymentListToAll`，修复 main 上列表断言超时（merge `ef356679` 回归）。

### Added

- **S1 协作切片（Web + REST）**：Issue 列表创建与筛选/游标一致；Code `code-commit-oid` + 扩展 Playwright；PR 评论 `FOR UPDATE` 与 sync 锁；负责人须 **repo grant**；Issue 详情 i18n；并发 sync/comment E2E（**不含** Git-over-SSH 守护进程）。
- `s1_collaboration_http_e2e`、`web/e2e/s1-collaboration.spec.ts`。
- 产品垂直切片：`musehub-app`（登录/会话、bare 仓库、Git smart HTTP、Issue、**reference** Agent 运行）。
- `musehub-agent-adapter` + ADR-008；`docs/PROGRESS.md`；migrations `0003`/`0004`。
- GitHub `ci.yml`（可移植测试 + Postgres 集成）与 `evidence-macos.yml`（Colima 证据，非门禁）。
- `./scripts/dev-local-stack.sh`：本地 loopback `init` / `start` / `status` / `stop`（`MUSEHUB_BIND` / `MUSEHUB_DEV_API_URL` 一致、`pg_isready`、进程 `started_epoch` 认领、失败仅回收本次子进程）；`verify-dev-local-stack*.sh` 正/负例。Linux CI 已验；**macOS 未在 Agent 环境实测**。
- `scripts/lib/sed-portable.sh`：macOS/Linux `sed_inplace`；生产安装与 runbook 验证复用。
- **PR-01**（PR #39 草稿）：Pull sync CAS、no-op sync、MCP PR 锚点校验、`pr_anchor` 单测、控制台 stale 修订 UX。
- **ISS 控制台复审**（PR #39）：列表 `label` trim/校验、负责人下拉刷新、审计 `resource_type` 筛选；相关 HTTP/Playwright。
- **IAM 控制台**（PR #39）：组织默认仓库角色 UI；仓库设置授予角色选择 + 撤销 grant；`iam_grants_console_http_e2e` / `iam-grants-console.spec.ts`。

## [0.1.0-rc4]

### Fixed

- MCP 测试：默认禁止测试进程内 `pip`；新增 `scripts/setup-mcp-matrix-python-deps.sh`。
- 静态 `m0-rmcp-matrix.json` 不再伪造 `negotiated_protocol`；Python Bearer 负例与 HTTP 一致使用 `legacy` 模式。
- 增补 Bearer 门控、探针级 E2E mock 与 lib 单元测试。

## [0.1.0-rc3]

开发中版本，不是产品发布（`v0.1.0-rc2` Git 标签为历史推送，不代表本轮发布）。

### Fixed

- MCP 矩阵：改用 **rust-rmcp** 与 **Python mcp==2.3.0** 两种独立 SDK（禁止同 SDK 仅改 client 名称冒充双客户端）；Python HTTP 在 `spawn_blocking` 下运行以免阻塞 Tokio 探针服务。

### Added

- `crates/musehub-mcp-probe/python/`：`mcp_matrix_runner.py` + 锁定 `requirements.txt`。

## [0.1.0-rc2]

### Added

- 新增 `musehub-mcp-probe`：基于 rmcp `3.8.0` 的 M0 stdio + Streamable HTTP 探针（初版矩阵口径不足，见 rc3 修正）。

## [0.1.0-rc1]

### Added

- 建立 Rust workspace：`musehub-core`、`musehub-server`、`musehub-git-probe`。
- 以根目录 `VERSION` 作为单一版本源，HTTP 响应携带 `X-App-Version`。
- 增加 `/healthz`、`/readyz`、`/version`。数据库与 Git 存储未接入时，`/readyz` 返回 503。
- 增加本地 Git 探针，验证 bare 仓库的 clone、非空 commit 和 push。
- 增加最小 React 页面，展示版本并提供中英文 i18n。
- 增加本地进程探针，验证工作目录约束、超时和取消。
- 增加本地 Agent fixture，验证事件契约、空 diff 拒绝和取消后无新副作用。
- 增加 Colima Docker 探针，按镜像 digest 启动 Compose，并核对实际镜像 ID 与 Docker socket 未挂载。
- 增加 Colima Linux VM 内的 rootless 探针，验证 rootless 身份、用户数据目录、BuildKit 构建和容器内无 Docker socket。
  - 增加 Colima Linux VM 内的 OpenSSH forced command 探针，验证服务端身份传递、命令白名单、并发和取消。这是单用户临时 sshd 证据，不传输真实 Git pack。
- 确认业务数据库只支持 PostgreSQL，不做其他数据库适配。
- 增加 `musehub-migrate` 和成对的 `0001_common`、`0002_identity` migration。隔离 PostgreSQL 17 已验证 up、down、up，以及身份表时间戳、更新触发器和外键索引。Web 启动不执行 migration；其余业务表尚未落地。

## [0.0.0-docs1] — 2026-10-30

### Added

- 初始化独立 Git 仓库，托管地址为 `github.com/niuwoai/musehub`，建立 `main` 与 `develop` 分支。
- 提交产品需求、技术架构、数据库、MCP、开发计划、竞品调研和文档检查报告。
- 添加 Rust/React 构建产物、本地环境配置及私钥文件的基础 Git 忽略规则。
- `v0.0.0-docs1` 仅标记文档快照，不代表已实现或发布产品。

## 设计基线 0.1 — 2026-10-30

### Added

- 确定独立项目名称 MuseHub，整理首版需求、技术架构、数据模型、MCP 契约、开发计划与竞品实践调研。
- 明确目标 `0.1.0-rc1` 是规划版本；截至该日期没有实现或发布。
