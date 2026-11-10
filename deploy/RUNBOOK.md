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

## TLS gateway (example)

`deploy/Caddyfile` documents `tls internal` + reverse proxy to API (`/mcp` included). Repeatable layout check: `scripts/verify-caddyfile-layout.sh` (static grep; `caddy validate` via Docker when available). Also runs at the start of `scripts/verify-product-stack-e2e.sh` and at the end of `scripts/verify-product-acceptance.sh`.

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
