# MuseHub product stack (deploy)

Clean Linux install uses **Docker Compose** only. The API container has **no** host `docker.sock` mount and runs subprocess adapters with fixed binaries under `/opt/musehub/bin`.

## Quick start (demo / lab stack)

```bash
cp deploy/.env.example deploy/.env
# Edit secrets (POSTGRES_PASSWORD, MUSEHUB_SESSION_SECRET, MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD)
cd deploy
docker compose up -d --build
```

Automated smoke (throwaway project, no worker): from repo root  
`./scripts/verify-clean-install-runbook.sh` → `clean_install_runbook=ok`.

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

Gateway serves **production static** `web/dist` (built in the gateway image) and proxies `/api`, `/git`, `/mcp` to `musehub-server:8080`.

- Lab HTTP: `deploy/Caddyfile.ci` (port 8080 on the gateway).
- TLS example: `deploy/Caddyfile` (`example.local`, `tls internal` — replace with real certs in production).

## Security initialization

| Variable | Purpose |
|----------|---------|
| `MUSEHUB_SESSION_SECRET` | Cookie signing (≥32 chars, unique per install) |
| `MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD` | First-boot `admin` user (used once at migrate/prepare) |
| `POSTGRES_PASSWORD` | Database password |

Do not commit `deploy/.env`. Rotate secrets on compromise; backup before destructive ops.

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

Migrations run on every server container start (idempotent `up` via `deploy/docker-entrypoint.sh` — same binary as manual `musehub-migrate up`).

Manual migrate only (compose):

```bash
docker compose run --rm --entrypoint /docker-entrypoint.sh musehub-server migrate-only
```

Host / CI parity (Postgres URL, down then up idempotency): `scripts/verify-product-acceptance.sh` with `DATABASE_URL` or `MUSEHUB_TEST_DATABASE_URL`.

## HTTP Git clone URL

The API listens on `0.0.0.0:8080` inside the container; the UI **`clone_url`** uses `MUSEHUB_PUBLIC_HTTP_ORIGIN` (default `http://127.0.0.1:<gateway-port>`). For real clients, set e.g. `MUSEHUB_PUBLIC_HTTP_ORIGIN=https://musehub.internal` in `deploy/.env` to match how users reach the **gateway** (`/git/...` is proxied).

## SSH Git transport (product)

Product Git-over-SSH is a **separate** listener (`musehub-git-ssh serve`), not the HTTP gateway port. OpenSSH invokes `musehub-git-ssh authorized-keys` and forces `musehub-git-ssh gateway` (no shell, no forwarding). Repository API returns **`ssh_clone_url`** (`MUSEHUB_PUBLIC_SSH_HOST`, `MUSEHUB_GIT_SSH_PORT`, default `2222`). Host keys persist under `MUSEHUB_GIT_SSH_RUN_DIR` (default `/var/lib/musehub/git-ssh`). Trust is the host key users pin on first connect — document your rotation procedure. Linux acceptance installs `openssh-server` when missing (`verify-product-acceptance.sh`). This is **not** untrusted multi-tenant process isolation and **not** the Colima `musehub-ssh-probe` harness.

```bash
export DATABASE_URL=...
export MUSEHUB_GIT_ROOT=/var/lib/musehub/git
export MUSEHUB_GIT_SSH_PORT=2222
export MUSEHUB_GIT_SSH_RUN_DIR=/var/lib/musehub/git-ssh
target/debug/musehub-git-ssh serve
```

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

**Not accepted in this repo:** real public DNS, Let's Encrypt issuance, or trust-store validation. CI only checks rendered layout + `caddy validate` with **fixture** certificates (`scripts/verify-caddyfile-layout.sh`).

`deploy/Caddyfile` remains a design reference (`tls internal`). Repeatable checks also run at the start of `scripts/verify-product-stack-e2e.sh` and at the end of `scripts/verify-product-acceptance.sh`.

## Backup and restore

From repo root (host tools `pg_dump` / `pg_restore`):

```bash
export DATABASE_URL=postgres://musehub:$POSTGRES_PASSWORD@127.0.0.1:55432/musehub   # use compose publish or exec
export MUSEHUB_GIT_ROOT=/path/to/extracted/git
export MUSEHUB_ARTIFACT_ROOT=/path/to/extracted/artifacts
./scripts/backup-musehub-state.sh /tmp/musehub-backup
```

CI stack test automates backup/restore against the compose Postgres port overlay (`deploy/docker-compose.ci.yml`).

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
