# Production MCP client compatibility matrix

> Scope: production `/mcp` Streamable HTTP endpoint. This matrix records automated SDK interoperability coverage; it does not claim validation of GUI hosts or OAuth flows.

## Covered clients

| Client implementation | SDK version | Transport | Negotiation | Checks |
|---|---:|---|---|---|
| Rust `rmcp` client | 3.8.0 | Streamable HTTP | Prefer `2026-07-28`, fall back to `2025-11-25` | Initialize; list all advertised tools; verify capabilities scope and resource templates; call repository list with a PAT; cleanly cancel client session |
| Official Python MCP SDK | 2.3.0 | Streamable HTTP | SDK auto negotiation | Initialize; list tools; verify every advertised tool; call repository list with a PAT; reject an unknown tool; reconnect and list tools again |

The tests create an isolated product app, PostgreSQL schema, repository, and PAT. The Rust case is `product_mcp_rust_streamable_http_matrix`; the Python case is `product_mcp_python_streamable_http_matrix` in `crates/musehub-app/tests/production_mcp_matrix.rs`. The Python runner is `crates/musehub-app/python/mcp_product_matrix_runner.py`.

## Run

Prepare the pinned Python SDK dependencies, set `MUSEHUB_TEST_DATABASE_URL` to a disposable PostgreSQL database, then run:

```bash
./scripts/setup-mcp-matrix-python-deps.sh
MUSEHUB_TEST_DATABASE_URL=postgres://musehub:musehub@localhost:5432/musehub cargo test -p musehub-app --test production_mcp_matrix -- --test-threads=1
```

The Python case fails with setup instructions if `python3`, `mcp`, or `httpx2` is unavailable. Missing dependencies are not reported as a passing compatibility check. CI provisions the SDK before the production security integration suite.

## Limits

- This covers SDK implementations against the production HTTP endpoint, not named GUI products such as Claude Desktop or IDE clients.
- Production stdio is not exposed or covered by this matrix; the separate M0 probe matrix covers stdio and HTTP against a probe server only.
- Remote OAuth, third-party host UX, and client-specific consent flows are outside the V1 compatibility claim.
- Write idempotency and authorization are covered by separate production MCP acceptance tests, not inferred from this transport matrix.
