# MuseHub product stack (deploy)

Clean Linux install uses **Docker Compose** only. The API container has **no** host `docker.sock` mount and runs subprocess adapters with fixed binaries under `/opt/musehub/bin`.

## Quick start

```bash
cp deploy/.env.example deploy/.env
# Edit secrets (POSTGRES_PASSWORD, MUSEHUB_SESSION_SECRET, MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD)
cd deploy
docker compose up -d --build
```

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

Migrations run on every server container start (idempotent `up`).

Manual migrate only:

```bash
docker compose run --rm --entrypoint /docker-entrypoint.sh musehub-server migrate-only
```

## Upgrade

```bash
docker compose build musehub-server gateway
docker compose up -d --force-recreate
```

If a migration fails, logs appear in `docker compose logs musehub-server`. Fix forward with a new image; restore from backup if data is inconsistent (see backup).

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

In-process **supervisor** workers are not a separate remote runner service. Pipeline/deploy adapters are **child processes** of the API container (`compose-mock` by default in the product image), not arbitrary shell from HTTP.

`GET /readyz` reports `checks.compose_adapter_mode` (`mock` | `runner` | `auto`) and `checks.pipeline_executor` (`in_process` | `remote`). The product image sets `MUSEHUB_COMPOSE_ADAPTER_MODE=mock`. For verified compose-runner + OCI, set `MUSEHUB_COMPOSE_ADAPTER_MODE=runner`, mount Docker, and follow `scripts/verify-pipeline-oci-product-e2e.sh` / `scripts/verify-compose-runner-e2e.sh`.

## What is not in this stack

- External OCI registry / compose-runner (use dev binaries + `verify-compose-runner-e2e.sh`)
- Live Codex (requires `MUSEHUB_AGENT_CODEX_BIN` + credentials; product image ships `codex-sim` only via explicit `adapter=codex-sim`)
