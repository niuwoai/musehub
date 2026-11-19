# MuseHub product stack (deploy)

Clean Linux install uses **Docker Compose** only. The API container has **no** host `docker.sock` mount and runs subprocess adapters with fixed binaries under `/opt/musehub/bin`.

## Quick start (demo / lab stack)

```bash
install -m 600 deploy/.env.example deploy/.env
# Edit secrets (POSTGRES_PASSWORD, MUSEHUB_SESSION_SECRET, MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD)
cd deploy
docker compose up -d --build
```

For an existing `deploy/.env`, run `chmod u=rw,go= deploy/.env` before editing or starting services. Keep the file owned by the account that manages the Compose stack.

Automated smoke (throwaway project, no worker): from repo root  
`./scripts/verify-clean-install-runbook.sh` → `clean_install_runbook=ok`.

## Optional GitHub OAuth App login

Create a GitHub OAuth App with the public MuseHub URL as its homepage and this exact authorization callback URL:

`https://<musehub-host>/api/v1/auth/github/callback`

For loopback development, HTTP callbacks on `localhost`, `127.0.0.1`, or `::1` are accepted. Production callbacks must use HTTPS. Put the OAuth App client ID, client secret, and matching callback URI in `deploy/.env` as `MUSEHUB_GITHUB_OAUTH_CLIENT_ID`, `MUSEHUB_GITHUB_OAUTH_CLIENT_SECRET`, and `MUSEHUB_GITHUB_OAUTH_REDIRECT_URI`, then recreate the server container so it reads the configuration. The sign-in page exposes “Continue with GitHub” when all three values are configured. First login creates a separate MuseHub human principal keyed by the GitHub numeric account ID; it does not link by email or grant organization membership automatically. Real OAuth App credentials and a deployed callback have not yet been accepted in this repository.

## Production profile (API enqueue-only + `musehub-runner`)

Control plane **must not** mount `docker.sock`. Pipeline / agent / deploy run on **`musehub-runner`** with a **selected PAT** (`deploy:execute`, `pipeline:execute`, `agent:execute`).

```bash
./scripts/install-production-compose.sh          # generates deploy/.env if missing; starts postgres+server+gateway
./scripts/configure-production-runner-pat.sh --start   # mint selected PAT → deploy/.env → start musehub-runner
```

Manual PAT (UI/API) remains supported: set `MUSEHUB_RUNNER_PAT` in `deploy/.env`, then  
`cd deploy && docker compose -f docker-compose.yml -f docker-compose.production.yml up -d musehub-runner`.

Full acceptance on a dev machine with Docker:  
`./scripts/verify-product-production-stack-e2e.sh` → `product_production_stack_e2e=ok`.

### Shared storage and separate-host Runner (experimental)

`docker-compose.shared-storage.yml` mounts operator-managed shared Git/artifact filesystems into the control plane, SSH service, and Runner. Set `MUSEHUB_GIT_ROOT_HOST` and `MUSEHUB_ARTIFACT_ROOT_HOST` to the same authoritative mounts on each control-plane node. Start the production stack with both overlays:

```bash
docker compose -f docker-compose.yml -f docker-compose.production.yml -f docker-compose.shared-storage.yml up -d --build
```

The filesystem must provide cross-host visibility, reliable POSIX locking, atomic same-directory rename, and persistence across node restart. After mounting it on two independent hosts, run `bash scripts/verify-shared-storage.sh publish <run-id> <host-a> <host-b>` on both hosts, then run the matching `verify` command on both. To verify cross-host advisory locking, run `bash scripts/verify-shared-storage.sh lock-hold <lock-run-id> <host-a> <host-b> 90` on host A, then run `bash scripts/verify-shared-storage.sh lock-check <lock-run-id> <host-b> <host-a>` on host B while the first command reports its locks are held; repeat with host roles reversed. Use fresh alphanumeric run IDs. The script leaves markers and lock files under `.musehub-shared-storage-probe/<run-id>` so evidence can be inspected and removed under the site's normal change procedure.

To check service from a surviving host after a source host is stopped, first publish the peer markers on both hosts and confirm ordinary `verify` passes. Start `lock-hold <run-id> <source-host> <survivor-host> 300` on the source host and wait for the “locks held” message. Fence or stop that host, then run `bash scripts/verify-shared-storage.sh survivor-check <run-id> <survivor-host> <source-host>` on the survivor. This checks that the survivor can read the source markers, atomically publish its own Git/artifact markers, and acquire both locks after the source process exits. Repeat with host roles reversed and a fresh run ID. This is a host-loss acceptance for the mounted filesystem; it does not create or prove storage replication, regional durability, or automatic failover. Remove test markers only after retaining evidence and under the site's normal change procedure.

#### Asynchronous one-way storage mirror (experimental)

For hosts without a shared POSIX filesystem, `scripts/mirror-musehub-storage.sh` provides a one-way Git/artifact mirror over SSH and rsync. Install rsync and OpenSSH on both hosts. Provision a dedicated destination root on the standby host, writable by the mirror SSH account, with `git`, `artifacts`, and `.musehub-mirror-status` subdirectories. Use matching numeric group IDs and mount permissions for the MuseHub containers on both hosts. Create `.musehub-mirror-target` in that root with the exact contents `musehub-storage-mirror-target-v1`; the script checks this marker before any transfer or deletion. Provision the SSH host key out of band and use a key available to the systemd service account. The script requires strict host-key checking and noninteractive SSH.

On region B, pre-create a dedicated destination root and its real (non-symlink) `git`, `artifacts`, and `.musehub-mirror-status` directories. Grant the mirror account write access only to those subdirectories; keep the root and `.musehub-mirror-target` marker read-only to that account. Create the marker with exact contents `musehub-storage-mirror-target-v1`. Configure `/etc/musehub/storage-mirror.env` with the non-secret source paths and destination. Before installation, provision the host account `musehub` and group `musehub-git` with IDs and filesystem permissions compatible with the container storage; the unit runs as `musehub:musehub-git`, and the installer fails if either is missing. Provision that account's read access to the source roots and its SSH key/known-hosts out of band. The destination account must have write access to the dedicated mirror subdirectories.

```bash
sudo install -d -o root -g root -m 0755 /etc/musehub
sudo install -o root -g root -m 0600 deploy/storage-mirror.env.example /etc/musehub/storage-mirror.env
sudoedit /etc/musehub/storage-mirror.env
```

The config must be a root-owned regular file and must not be group/world writable. The installer reads only its metadata, installs the mirror script and systemd units, then reloads systemd. Install units on both hosts if needed for recovery, but enable the timer on the primary only:

```bash
sudo ./scripts/install-musehub-storage-mirror.sh
sudo ./scripts/install-musehub-storage-mirror.sh --enable  # run on the primary only
sudo systemctl list-timers musehub-storage-mirror.timer
sudo systemctl status musehub-storage-mirror.service
```

The timer runs every minute. Successful runs checksum both trees, copy changed files with per-file delayed replacement, and publish `.musehub-mirror-status/latest` only after both transfers succeed. This reads file contents on each pass, so account for source-storage I/O when setting the schedule. Failed passes leave the previous success timestamp; alert on service failures and on a status age above the site's RPO budget. This is asynchronous file replication: it has a nonzero RPO, does not replicate PostgreSQL, does not give a cross-component consistent snapshot, and does not support simultaneous writers. The continuous mode does not propagate deletions. Do not expose the standby as writable while the primary can still write.

Run `scripts/verify-storage-mirror-local-e2e.sh` to exercise same-size file updates, continuous-mode deletion protection, checksum drift detection, required final-sync acknowledgements, deletion convergence, and checkpoint behavior in a disposable Alpine Docker container. It tests the explicitly gated local target mode, SSH/rsync to an sshd process inside that isolated container, PostgreSQL 17 streaming replication between two local instances, barrier lag rejection, local standby promotion, and the barrier migration down script. It does not exercise a separate host, systemd timer, remote RPO, physical host/region failover, CloudNativePG promotion, fencing, or traffic cutover.

On 2026-11-17, an isolated Colima VM with systemd 255 installed and ran the real MuseHub mirror service/timer against the guarded local target. The first run and the run after `systemctl reboot` both succeeded, wrote the mirror checkpoint, and produced source/destination files with matching checksums. This validates the unit and guest reboot path in one VM; it does not validate remote SSH replication, a physical host reboot, remote RPO, fencing, or regional cutover.

For a planned cross-region switchover, first put MuseHub in maintenance and stop all MuseHub, SSH, Runner, scheduled-job, and operational writers on the primary. Confirm application writes are quiesced; the acknowledgement below is an operator assertion, not an automated writer fence. Load the protected, non-secret mirror path configuration as the configured mirror account:

```bash
set -a
. /etc/musehub/storage-mirror.env
set +a
export MUSEHUB_STORAGE_SOURCE_QUIESCED=YES
```

On region A, create the PostgreSQL barrier only after writes have stopped. The helper requires a writable primary, inserts a durable UUID row, then records the WAL LSN. Set `MUSEHUB_DR_DATABASE_URL` from the protected operator environment without echoing it, then capture the printed ID and LSN for the DR verification step:

```bash
export MUSEHUB_DR_DATABASE_URL='<region-A writable primary connection URL>'
barrier_output="$(/usr/local/libexec/musehub/create-musehub-dr-barrier.sh)"
export MUSEHUB_STORAGE_MIRROR_BARRIER_ID="$(printf '%s\n' "$barrier_output" | awk -F= '$1 == "dr_barrier_id" { print $2 }')"
export MUSEHUB_STORAGE_MIRROR_BARRIER_LSN="$(printf '%s\n' "$barrier_output" | awk -F= '$1 == "dr_barrier_lsn" { print $2 }')"
printf '%s\n' "$barrier_output"
```

Check the dedicated destination root and its marker, then run the final checksum pass and dry-run verification. Final mode propagates deletions and is destructive within the marked destination `git/` and `artifacts/` trees:

```bash
export MUSEHUB_STORAGE_MIRROR_ALLOW_DELETE=YES
/usr/local/libexec/musehub/mirror-musehub-storage.sh quiesced-final
/usr/local/libexec/musehub/mirror-musehub-storage.sh verify
```

On region B, set the DR database URL, mirror status file path, and the barrier ID/LSN printed on region A. Run the verifier against the still-standby database; it requires the matching final mirror status, barrier row, and WAL replay through the recorded LSN:

```bash
export MUSEHUB_DR_DATABASE_URL='<region-B standby connection URL>'
export MUSEHUB_DR_MIRROR_STATUS_FILE='/srv/musehub-storage/.musehub-mirror-status/latest'
export MUSEHUB_DR_BARRIER_ID='<dr_barrier_id from region A>'
export MUSEHUB_DR_BARRIER_LSN='<dr_barrier_lsn from region A>'
/usr/local/libexec/musehub/verify-musehub-dr-barrier.sh
```

Proceed only when this reports `state=replica-ready`. Then follow the CloudNativePG demotion-token, old-primary fencing, and controlled-promotion steps below. After promotion, set `MUSEHUB_DR_DATABASE_URL` to the new writable endpoint and rerun this verifier, then run `scripts/verify-postgres-primary.sh` against the writer endpoint, former-primary management endpoint, and every API base. Confirm the old primary is fenced, point `MUSEHUB_GIT_ROOT_HOST` and `MUSEHUB_ARTIFACT_ROOT_HOST` at the standby copies, and start MuseHub/Runner on one host only after both verification steps pass. The barrier binds acknowledged PostgreSQL and file-mirror progress; it does not itself stop every writer, replicate PostgreSQL, fence either region, promote CloudNativePG, or shift API traffic. Preserve the barrier output, final mirror status, fencing and promotion evidence, and DB backup/restore evidence with the recovery record. Unplanned failover must accept the last completed mirror's RPO. Do not automatically fail back: re-seed the former primary from the new source under a separate, reviewed recovery procedure.

This is an opt-in shared-filesystem configuration, not a replicated storage implementation. `deploy/postgres-ha-cluster.yaml` provides a CloudNativePG single-region HA database cluster reference with three instances, synchronous commit durability, failover quorum, zone anti-affinity, and a private writer LoadBalancer service. It requires an existing three-zone Kubernetes cluster, the CloudNativePG operator, a durable RWO StorageClass, and a private LoadBalancer implementation. A local k3d acceptance on 2026-11-17 confirmed automatic promotion after deleting the primary Pod, recovery to 3/3 Ready, and retention of a synchronous-commit probe row on the promoted primary. A second acceptance stopped the k3d agent container hosting the primary; CloudNativePG promoted `musehub-postgres-2`, the cluster recovered to 2/3 Ready, the old worker container was `exited`, and the acknowledged row remained. Both experiments used local-path storage inside a single-machine Docker cluster; they do not verify real physical node loss, network partition fencing, cloud storage durability, writer/API traffic cutover, or disaster recovery. PostgreSQL streaming replication protects database state; Git/artifact replication is a separate storage-system responsibility.

Install the pinned CloudNativePG operator release, create the namespace and DB credential secret from your secret manager, then apply the cluster manifest:

```bash
kubectl apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
kubectl create namespace musehub --dry-run=client -o yaml | kubectl apply -f -
kubectl create secret generic musehub-db-credentials --namespace musehub --from-env-file=/path/to/protected-db-credentials.env --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f deploy/postgres-ha-cluster.yaml
kubectl get cluster musehub-postgres --namespace musehub --watch
```

The `fast-rwo` StorageClass must exist and provide zone-aware persistent volumes. Configure the generated `musehub-postgres-writer` LoadBalancer as private-only, install a TLS certificate trusted by MuseHub, and set `MUSEHUB_DATABASE_URL` to its stable DNS name. The manifest schedules members across three zones and uses synchronous replication with failover quorum; if the required synchronous standby is unavailable, database writes pause instead of acknowledging data without the configured durability. The Kubernetes control plane and worker nodes must themselves span independent failure domains and have quorum.

The external-database overlay remains useful for any operator-managed HA endpoint:

```bash
docker compose -f docker-compose.yml -f docker-compose.production.yml -f docker-compose.shared-storage.yml -f docker-compose.external-postgres.yml up -d --build
```

Set `MUSEHUB_DATABASE_URL` in the protected `deploy/.env` to the cluster's stable writer proxy/VIP/DNS endpoint with TLS certificate verification (for example, `sslmode=verify-full`). The endpoint must route to the writable leader and preserve session affinity; do not use transaction-pooling mode because migrations hold a session advisory lock for their full run. The overlay disables the embedded PostgreSQL service and points API, SSH, and Runner at that endpoint. `/readyz` reports `checks.database_role` as `primary`, `standby`, or `read_only` and is ready only on a writable primary. The API rejects mutation requests, including OIDC/SAML login starts, with HTTP 503 when its database endpoint is in recovery, read-only, or the role check fails. Startup refuses to run control-plane initialization on a read-only endpoint.

After the HA operator performs a planned or unplanned promotion, verify both the writer endpoint and every API instance. Set `MUSEHUB_DATABASE_URL` to the new writer, `MUSEHUB_HA_OLD_PRIMARY_DATABASE_URL` to a direct, separately routed connection to the former primary, and `MUSEHUB_HA_API_BASES` to a comma-separated list of API origins, then run `bash scripts/verify-postgres-primary.sh`. The check fails if the writer is read-only, the former primary remains writable, any API is not ready on a writable primary, or API versions disagree. The former-primary probe requires management-network access to that database; if the HA manager fences it at the network layer, retain and inspect the provider's fencing evidence separately. This is a post-failover check, not a failover simulator. Automatic failover, quorum, replication, and fencing are provided by the external PostgreSQL HA manager. PostgreSQL role checks cannot prevent split brain if two independent writable primaries remain reachable. In-flight requests can fail during promotion; retry only operations protected by the existing idempotency/CAS contracts.

HTTP/SSH receive-pack, PR merge, and AgentRun publication use PostgreSQL transaction-scoped repository locks, but direct filesystem Git changes bypass them. The `repository_lock_contention_e2e` now passes against a disposable PostgreSQL 17 container and verifies contention across independent pools. Physical multi-host write contention and long-running reboot acceptance remain outstanding.

### Multi-region PostgreSQL disaster recovery (experimental)

This reference uses two independent Kubernetes clusters, CloudNativePG, the Barman Cloud Plugin, and an object-store bucket in region B that is reachable from both regions. It is asynchronous archive-based disaster recovery; replication lag defines the possible data-loss window. It is not synchronous multi-region HA and does not automatically move MuseHub API traffic. Install compatible, pinned CloudNativePG and Barman Plugin releases in both clusters, create the `musehub` namespace, and provision the referenced object-store Secrets through the secret manager. Replace the example bucket destinations and image digest with the operator-approved values before applying.

On both Kubernetes contexts, apply `deploy/postgres-dr-object-stores.yaml` and provision the region-B object-store Secret in each cluster. The region-B bucket is the shared WAL archive for the primary and DR cluster; this reference does not configure a region-A archive. On region A, apply `deploy/postgres-dr-primary.patch.yaml` to the existing writable cluster, then wait until CloudNativePG reports a successful base backup and continuous WAL archiving. On region B, apply `deploy/postgres-dr-replica.yaml`. Its `replica.primary` and `replica.source` configure CloudNativePG distributed topology; `replica.enabled` is intentionally omitted. Keep the DR writer endpoint private and out of application DNS until promotion. Monitor replica recovery/lag and periodically restore a snapshot in an isolated cluster; a Ready standby alone does not prove the required recovery point is usable.

A planned switchover uses the CloudNativePG demotion token so promotion waits for the replica to reach the old primary's WAL position:

1. Put MuseHub and Runner clients into maintenance and stop new writes. Verify the DR replica has caught up.
2. On region A, demote the current primary by pointing it at the intended new primary:

   ```bash
   kubectl --context <region-a-context> patch cluster musehub-postgres-primary --namespace musehub --type=merge --patch '{"spec":{"replica":{"primary":"musehub-postgres-dr","source":"musehub-postgres-dr"}}}'
   ```

3. Wait for `status.demotionToken` on the old cluster and verify its database is no longer writable. Fence the old region's client traffic and write path using the infrastructure's network, load-balancer, or storage fencing mechanism. Keep the management-plane Kubernetes API reachable for the script to read the demotion token.
4. Request promotion from a controlled operator shell. The script checks the old/new cluster relationship, requires an explicit fencing acknowledgement, keeps the token out of command arguments and output, and patches the DR cluster with that token:

   ```bash
   MUSEHUB_DR_OLD_KUBECTL_CONTEXT=<region-a-context> \\
   MUSEHUB_DR_NEW_KUBECTL_CONTEXT=<region-b-context> \\
   MUSEHUB_DR_OLD_CLUSTER=musehub-postgres-primary \\
   MUSEHUB_DR_NEW_CLUSTER=musehub-postgres-dr \\
   MUSEHUB_DR_OLD_PRIMARY_FENCED=YES \\
   ./scripts/promote-cnpg-dr-replica.sh
   ```

5. Confirm CloudNativePG reports the new primary and the old cluster is a read-only replica. Move the private writer endpoint to region B, rerun `verify-musehub-dr-barrier.sh` against the promoted primary with the same barrier ID/LSN and region-B mirror status file, then run `scripts/verify-postgres-primary.sh` with the new writer URL, a management-path URL to the former primary, and every API base. Resume application writes only after all three checks pass.

The promotion helper is for a controlled switchover with a reachable old-cluster control plane and demotion token. It deliberately stops if the token is missing; do not bypass the check during an outage. Unplanned recovery requires first fencing the old region, then following the operator's documented archive/PITR recovery procedure and accepting the resulting RPO. The local acceptance verifies barrier creation, file mirror state, WAL replay gating, promotion, and post-promotion barrier presence inside one disposable container. No real multi-region cluster, cloud object store, network fencing, writer DNS change, API traffic shift, or systemd timer has been accepted in this environment.

Gateway serves **production static** `web/dist` (built in the gateway image) and proxies `/api`, `/git`, `/mcp` to `musehub-server:8080`.

- Lab HTTP: `deploy/Caddyfile.ci` (port 8080 on the gateway).
- TLS example: `deploy/Caddyfile` (`example.local`, `tls internal` — replace with real certs in production).

## Security initialization

| Variable | Purpose |
|----------|---------|
| `MUSEHUB_SESSION_SECRET` | Cookie signing (≥32 chars, unique per install) |
| `MUSEHUB_WEBHOOK_ENCRYPTION_KEY` | Optional until Webhooks are configured; base64-encoded 32-byte AES-256-GCM key, keep separate from database backups |
| `MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD` | First-boot `admin` user (used once at migrate/prepare) |
| `POSTGRES_PASSWORD` | Database password |

Do not commit `deploy/.env`. The production installer and Runner PAT configurator reject symlink paths and enforce mode `0600`; for a manually created or existing file, run `chmod u=rw,go= deploy/.env` and keep it owned by the account that manages the Compose stack. Rotate secrets on compromise; backup before destructive ops.

## Boot persistence (systemd)

After a production install (`deploy/.env` present, runner PAT optional):

```bash
sudo ./scripts/install-musehub-systemd.sh
sudo systemctl enable --now musehub-control-plane.service
sudo systemctl enable musehub-runner.service   # unit enabled; worker starts only when PAT is set
```

Units **After=docker.service**; secrets remain in `deploy/.env` only. Re-run `install-musehub-systemd.sh` if you move the git checkout. If you **enable** `musehub-runner` before `MUSEHUB_RUNNER_PAT` is set, the start unit exits cleanly without starting a worker; after you configure the PAT (e.g. `configure-production-runner-pat.sh`), run `sudo systemctl restart musehub-runner` — **enable alone does not retry** a previously skipped worker start. **Reboot after enable has not been validated in CI** — validate on your host before calling it production-ready.

## Persistence

Named volumes: `pgdata`, `gitdata`, `artifacts`. Survive `docker compose restart` and image upgrades when volumes are retained.

```bash
docker compose restart musehub-server gateway
```

## Migrations

Migrations run on every server container start (idempotent `up` via `deploy/docker-entrypoint.sh` — same binary as manual `musehub-migrate up`). `musehub-migrate` takes a PostgreSQL session advisory lock for the complete up/down operation, so simultaneous server starts serialize schema changes; if the process is canceled, its dedicated lock connection is closed so PostgreSQL releases the lock.

Manual migrate only (compose):

```bash
docker compose run --rm --entrypoint /docker-entrypoint.sh musehub-server migrate-only
```

Host / CI parity (Postgres URL, down then up idempotency): `scripts/verify-product-acceptance.sh` with `DATABASE_URL` or `MUSEHUB_TEST_DATABASE_URL`.

## HTTP Git clone URL

The API listens on `0.0.0.0:8080` inside the container; the UI **`clone_url`** uses `MUSEHUB_PUBLIC_HTTP_ORIGIN` (default `http://127.0.0.1:<gateway-port>`). For real clients, set e.g. `MUSEHUB_PUBLIC_HTTP_ORIGIN=https://musehub.internal` in `deploy/.env` to match how users reach the **gateway** (`/git/...` is proxied).

## SSH Git transport (product)

Product Git-over-SSH is a **separate** listener (`musehub-git-ssh serve` or the **`musehub-git-ssh`** Compose service), not the HTTP gateway port. OpenSSH runs **`AuthorizedKeysCommand`** (`musehub-git-ssh authorized-keys`) as the `git` OS user and terminates into the MuseHub **login shell** (`/usr/local/lib/musehub/musehub-git-login.sh` → `musehub-git-ssh shell` → gateway). The authorized_keys line sets **`MUSEHUB_SSH_KEY_ID`** and **`MUSEHUB_SSH_KEY_FP`** (no `command=` override).
Gateway uses **`MUSEHUB_SSH_KEY_FP`** from the AuthorizedKeysCommand line for the authenticating key; trustworthy only when **`git` passwd home** (`pw_dir`, not session `HOME`) is **`/var/lib/musehub/git-ssh/home`** — root-owned `0555`, no `~/.ssh/environment`. Database credentials live in **`runtime.env`** under `MUSEHUB_GIT_SSH_RUN_DIR` (mode `0640`, `root:git`), not in world-readable wrapper scripts. Bare repos on the shared `gitdata` volume are **`musehub:musehub-git`** with setgid so the `git` transport user can `receive-pack`. Repository API and the Code tab show **`ssh_clone_url`** (`MUSEHUB_PUBLIC_SSH_HOST`, `MUSEHUB_GIT_SSH_PORT`, default `2222`).

| Path / variable | Purpose |
|----------------|---------|
| `deploy/docker-compose.yml` → `musehub-git-ssh` | Product sshd sidecar; shares `gitdata` with API |
| Volume `gitsshdata` | Persistent **host keys** + sshd pid/config under `MUSEHUB_GIT_SSH_RUN_DIR` |
| `MUSEHUB_GIT_SSH_LISTEN` | Bind address (`0.0.0.0` in Compose; `127.0.0.1` for local acceptance) |
| `deploy/.env` | Set `MUSEHUB_PUBLIC_SSH_HOST` to the hostname users type for `ssh://git@…` |

**Trust & rotation:** Users pin the sshd host public key on first connect (`gitsshdata` or `MUSEHUB_GIT_SSH_RUN_DIR`). To rotate, stop sshd, archive old keys, let `serve` generate new host keys (or run `ssh-keygen` manually), publish the new fingerprint to your team, then restart. **Permissions:** `musehub-git-ssh serve` starts **sshd as root**; interactive OS login is only the restricted MuseHub login shell for user `git`. The ssh run dir defaults to **`0750` `root:git`** (group traverse, no world access); `runtime.env` is **`0640` `root:git`**. Login shell and AKC wrapper hard-code `/usr/local/bin/musehub-git-ssh` and fail closed if `runtime.env` is unreadable. Bare repos use **`musehub-git`** + `core.sharedRepository=group`. Product acceptance includes **`verify-product-compose-git-ssh-e2e.sh`** on the full Compose stack (API + `musehub-git-ssh`, default volume modes).

**systemd:** optional `musehub-git-ssh.service` (installed by `install-musehub-systemd.sh`) starts/stops the Compose sshd service with the control plane — secrets stay in `deploy/.env` only.

CI **fails** product acceptance when `openssh-server` is missing (`MUSEHUB_REQUIRE_GIT_SSH=1`); local runs without sshd exit the SSH suites as `not_run` (never as false success). This is **not** untrusted multi-tenant process isolation and **not** the Colima `musehub-ssh-probe` harness.

## Upgrade

Production profile (rebuild, migrate, recreate control plane + runner when PAT is set):

```bash
./scripts/upgrade-production-compose.sh
```

Manual (demo profile):

```bash
docker compose build musehub-server gateway
docker compose up -d --force-recreate
```

Images recreate containers; **named volumes** (`pgdata`, `gitdata`, `artifacts`) keep data when migrations succeed. If a migration fails, logs appear in `docker compose logs musehub-server`. Fix forward with a new image; restore from backup if data is inconsistent (see backup).

## TLS gateway (production)

Lab/CI keeps **HTTP** on `deploy/Caddyfile.ci` (`:8080`). Production install uses a rendered `deploy/Caddyfile.live` (same routes: API, `/git`, `/mcp`, static UI).

### Enable HTTPS

1. Edit `deploy/.env` (see `.env.example`):

   | Variable | Purpose |
   |----------|---------|
   | `MUSEHUB_GATEWAY_TLS_MODE` | `off` (default HTTP), `manual` (your cert/key), or `acme` (Caddy automatic HTTPS) |
   | `MUSEHUB_GATEWAY_TLS_DOMAIN` | Public DNS name users type in the browser (required when TLS is on) |
   | `MUSEHUB_ACME_EMAIL` | Contact for ACME (`acme` mode) |
   | `MUSEHUB_TLS_CERT_FILE` / `MUSEHUB_TLS_KEY_FILE` | Host paths to PEM files (`manual` mode; default `deploy/tls/cert.pem` + `key.pem`) |
   | `MUSEHUB_GATEWAY_HTTPS_PORT` | Host publish for HTTPS (default `443`) |
   | `MUSEHUB_GATEWAY_HTTP_REDIRECT_PORT` | Host `:80` for HTTP→HTTPS redirect and ACME HTTP-01 (`acme` mode) |

2. Place cert/key files for **manual** mode, then render (fail-closed if domain is missing/placeholder or files absent):

   ```bash
   ./scripts/render-gateway-caddyfile.sh
   ```

3. Recreate the gateway: `./scripts/upgrade-production-compose.sh` (or `production_compose up -d --force-recreate gateway` from `deploy/` after sourcing `scripts/lib/production-compose.sh`).

4. Set **`MUSEHUB_PUBLIC_HTTP_ORIGIN=https://<your-domain>`** (same host/scheme as the TLS front door) so Git **`clone_url`** in the UI stays correct.

Run the live check from a machine outside the deployment network after DNS and TLS are active:

```bash
MUSEHUB_PUBLIC_TLS_DOMAIN=musehub.example.com MUSEHUB_PUBLIC_TLS_EXPECTED_VERSION=0.12.0-rc1 ./scripts/verify-public-tls.sh
```

The script checks DNS resolution from that machine, HTTP-to-HTTPS redirect, trusted certificate chain and hostname (through the system `curl` trust store), `/readyz`, and agreement between `/readyz`, `/version`, and `X-App-Version`. For nonstandard ports, set `MUSEHUB_PUBLIC_TLS_HTTP_PORT` and `MUSEHUB_PUBLIC_TLS_HTTPS_PORT`. Success is specific to the host and network where it ran; retain the command output with deployment evidence. CI still uses fixture certificates (`scripts/verify-caddyfile-layout.sh`), and no real public DNS/ACME run has yet been recorded for this project.

`deploy/Caddyfile` remains a design reference (`tls internal`). Repeatable checks also run at the start of `scripts/verify-product-stack-e2e.sh` and at the end of `scripts/verify-product-acceptance.sh`.

### Configure OIDC sign-in

OIDC is optional and supports one provider per deployment. Set these values in `deploy/.env`:

- `MUSEHUB_OIDC_ISSUER_URL`: the provider's exact issuer URL from its discovery metadata.
- `MUSEHUB_OIDC_CLIENT_ID` and optional `MUSEHUB_OIDC_CLIENT_SECRET`.
- `MUSEHUB_OIDC_REDIRECT_URI`: register this exact URL at the provider. It must use HTTPS, except `http://localhost` or loopback for local development.
- `MUSEHUB_COOKIE_SECURE=1` when the public UI uses HTTPS. An HTTPS OIDC callback enables Secure session cookies automatically.

Restart the server after changing provider settings. Startup performs OIDC discovery and fails product API initialization if the configured issuer cannot be reached or validated. OIDC uses Authorization Code + PKCE, one-time state and nonce, a browser binding cookie, and ID token signature/issuer/audience/expiry validation. Login accepts only a previously mapped `(issuer, subject)` and an active human principal. It never maps by email or grants organization access from IdP claims.

An administrator provisions the principal through the normal MuseHub account/member process, then binds the provider subject to that existing handle. Use the canonical issuer and subject from the provider's directory; the returned row must identify exactly one principal:

```sql
INSERT INTO external_identities (id, principal_id, issuer, subject)
SELECT gen_random_uuid(), p.id, :'issuer', :'subject'
FROM principals p
WHERE p.handle_normalized = lower(:'handle')
  AND p.kind = 'human'
  AND p.status = 'active'
RETURNING id, principal_id;
```

For example, run this statement with `psql "$DATABASE_URL" -v issuer='https://idp.example.com/' -v subject='provider-subject' -v handle='alice'`. Review the returned principal before allowing the user to sign in. A subject already bound to another principal causes the unique constraint to reject the change. To remove access, delete the external identity mapping or disable the principal through the normal admin flow; disabling also invalidates existing sessions.

The callback creates the same revocable database-backed session as password login. OIDC login does not create a principal, organization, membership, owner role, or account link. SAML is separately configured as described below; OAuth authorization for remote MCP clients remains unsupported. Real OIDC and SAML interoperability have not yet been accepted against an operator's provider.

### Configure SAML SSO

SAML is an optional SP-initiated SSO integration. Configure all five values in `deploy/.env` and run Compose with the `docker-compose.saml.yml` overlay. The overlay mounts the IdP metadata XML and its separately pinned metadata-signing certificate read-only; startup fails if metadata signature verification, entity ID checks, endpoint parsing, or certificate validation fails.

```dotenv
MUSEHUB_SAML_IDP_ENTITY_ID=https://idp.example.com/entity
MUSEHUB_SAML_IDP_METADATA_HOST_PATH=/etc/musehub/saml/idp-metadata.xml
MUSEHUB_SAML_METADATA_SIGNING_CERT_HOST_PATH=/etc/musehub/saml/metadata-signing-cert.pem
MUSEHUB_SAML_SP_ENTITY_ID=https://musehub.example.com/saml/metadata
MUSEHUB_SAML_ACS_URL=https://musehub.example.com/api/v1/auth/saml/acs
```

Both host files must be readable by Docker and are mounted read-only. The IdP metadata must carry a valid XML signature verifiable by the pinned PEM certificate; unsigned metadata is rejected. The public ACS URL must use HTTPS. Start the stack with the same compose files used by the deployment plus the SAML overlay, for example:

```bash
cd deploy && docker compose -f docker-compose.yml -f docker-compose.production.yml -f docker-compose.saml.yml up -d --build musehub-server
```

Register `/api/v1/auth/saml/metadata` as the SP metadata URL at the IdP and allow the POST ACS URL above. The login page shows the SAML button only when configuration loads successfully. MuseHub accepts SP-initiated POST responses only; unsolicited IdP-initiated responses are rejected. Relay state, browser binding, request correlation, response IDs, and assertion IDs use database-backed, single-use/unique storage so multiple server processes share replay protection. The login identity is `(assertion issuer, NameID)` and must be pre-bound to an existing active human principal through `external_identities`; email claims never create or claim an account. Real enterprise IdP interoperability still needs operator acceptance. The selected SAML crate is pre-1.0 and does not declare an external security audit, so treat this integration as experimental until it passes an independent security review and the target IdP's interoperability suite.

### Configure SCIM 2.0 user provisioning

SCIM is an organization-scoped Users API subset. An organization owner or admin creates an expiring credential through the authenticated admin API; the plaintext token is returned only once. Store it in the IdP secret store. Tokens are scoped to one organization, are stored hashed, and can be listed by prefix/expiry or revoked. Creation and revocation are recorded in the organization audit log without storing the plaintext token. A token stops working if it expires, is revoked, or its creating principal is disabled.

```bash
curl -X POST 'https://musehub.example.com/api/v1/orgs/acme/scim-tokens' \\
  -H 'Authorization: Bearer <admin-session>' \\
  -H 'Content-Type: application/json' \\
  --data '{"expires_in_days":90}'
```

The response contains the token once. To inspect registered token prefixes and revoke a credential:

```text
GET    /api/v1/orgs/{org}/scim-tokens
DELETE /api/v1/orgs/{org}/scim-tokens/{token_id}
```

Configure the IdP base URL as `https://musehub.example.com/api/v1/scim/v2/organizations/acme`, with the returned bearer token. Service discovery is available at `/ServiceProviderConfig`, `/Schemas`, and `/ResourceTypes`; user resources are under `/Users` and support list, read, create, PUT, PATCH, and DELETE. Pagination and single-condition equality filters on `userName`, `externalId`, `displayName`, and `active` are supported. PATCH supports `add`, `replace`, and path-qualified `remove` for `active` and `displayName`. Sorting, Bulk, Groups, and password sync are not supported; inspect the advertised capabilities before enabling provisioning.

SCIM-created accounts receive an active membership with the `member` role and no password credential. `externalId` is stored per organization and is never used to claim an existing account or create an OIDC mapping. Configure a supported sign-in identity separately before expecting a provisioned user to log in. SCIM deactivation changes only the membership in that organization; it does not disable the global principal. SCIM cannot deactivate an owner. Handler errors use the RFC 7644 Error schema and `application/scim+json`; malformed requests rejected by Axum extractors and real enterprise IdP interoperability still require validation.

## Backup, off-host replication, and restore

From repo root (host tools `pg_dump` / `pg_restore`):

```bash
export DATABASE_URL=postgres://musehub:$POSTGRES_PASSWORD@127.0.0.1:55432/musehub   # use compose publish or exec
export MUSEHUB_GIT_ROOT=/path/to/extracted/git
export MUSEHUB_ARTIFACT_ROOT=/path/to/extracted/artifacts
./scripts/backup-musehub-state.sh /tmp/musehub-backup
./scripts/replicate-musehub-backup.sh /tmp/musehub-backup s3:company-dr/musehub "$(date -u +%Y%m%dT%H%M%SZ)"
```

Configure the `s3:company-dr` rclone remote with the provider's secret manager or instance identity; do not place access keys in shell history or the repository. The replication command creates a new snapshot directory, never deletes remote objects, refuses to overwrite existing objects with different content, then downloads and compares every copied file. Retain the snapshot ID and provider-side object versioning/retention policy. Materialize a selected snapshot into a new, nonexistent local directory before any restore:

```bash
./scripts/materialize-musehub-backup.sh s3:company-dr/musehub/<snapshot-id> /path/to/restore-input
```

The command downloads to a temporary sibling directory, compares downloaded bytes with the remote prefix, validates manifest entries and archive paths, then renames the verified directory into place. Follow the restore procedure below only with an isolated target database and fresh Git/artifact paths: `restore-musehub-state.sh` replaces the target database schema and removes its configured storage directories. This provides off-host point-in-time backup recovery; it is not live filesystem replication, automatic failover, or a cross-host POSIX locking solution. Database dump and filesystem archives are captured sequentially, so schedule a write-quiescence window for a cross-component consistent recovery point.

CI stack test automates backup/restore against the compose Postgres port overlay (`deploy/docker-compose.ci.yml`). An additional isolated PostgreSQL 17.7 test completed local backup→restore and matched a database row, Git main OID, and artifact SHA. A local S3-compatible SeaweedFS acceptance verified snapshot upload and rclone download comparison (four files, zero differences), but downloading the snapshot prefix for restore returned `directory not found`. No real provider bucket or restore-from-remote acceptance has been completed in this environment.

## Failure recovery

1. `docker compose ps` — ensure `postgres` and `musehub-server` are healthy.
2. `docker compose logs musehub-server --tail=200` — migrate vs runtime errors.
3. Postgres volume corrupt: stop stack, restore `pgdata` from backup or re-run restore script.
4. Git/artifacts loss: restore tar archives from backup manifest.
5. Gateway 502: wait for `musehub-server` `/readyz`; gateway depends on server health.

## Runner boundary

| Profile | API `docker.sock` | Executors |
|---------|-------------------|-----------|
| Demo (`docker-compose.yml`) | **No** | In-process adapters; `compose-mock` default |
| Production (`docker-compose.production.yml`) | **No** | `MUSEHUB_*_EXECUTOR=remote`; worker **`musehub-runner`** holds `docker.sock` + adapter bins |

Worker env **must** include `MUSEHUB_COMPOSE_ADAPTER_MODE=runner` when `MUSEHUB_REQUIRE_PIPELINE_OCI=1`.

`GET /readyz` reports `compose_adapter_mode`, `pipeline_executor`, `agent_executor`, `deploy_executor`. See `docs/RUNNER-BOUNDARY.md`.

## What is not in this stack

- External OCI registry / compose-runner (use dev binaries + `verify-compose-runner-e2e.sh`)
- Live Codex (requires `MUSEHUB_AGENT_CODEX_BIN` + credentials; product image ships `codex-sim` only via explicit `adapter=codex-sim`)
