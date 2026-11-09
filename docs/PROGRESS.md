# MuseHub 首版进度（真实状态）

> 对照 `01-product-requirements.md` P0 与 README 验收主线。`[x]`=可真实操作；`[~]`=部分/探针/mock；`[ ]`=未实现。  
> 开发版本见根目录 `VERSION`。**不把探针或未跑 CI job 当作产品完备。**

## CI 门禁（须在 `develop` 上全绿）

| Job | 状态 | 证据 |
|-----|------|------|
| `rust-portable` | `[x]` | fmt/clippy/portable/acceptance/backup/安全 E2E |
| `web` | `[x]` | Playwright `scripts/run-web-e2e.sh`（失败时 `collect-e2e-diagnostics.sh`）；`MUSEHUB_PRODUCT_OCI_E2E=1` 时 delivery 经 pipeline OCI 产品路径 |
| `compose-runner-e2e` | `[x]` | `scripts/verify-compose-runner-e2e.sh`（真实 pull/up/inspect/rollback，**非** `docker info` 探针） |
| `pipeline-oci-product` | `[x]` | `scripts/verify-pipeline-oci-product-e2e.sh`（CI 本地 registry、双 source_oid→双 digest、`pipeline_oci_product_e2e`）；develop run [`42596872230`](https://github.com/niuwoai/musehub/actions/runs/42596872230) 全绿 |

## P0 清单（节选）

| ID | 状态 | 证据/限制 |
|---|---|---|
| REC-01 | `[x]` | 启动 `boot_reconcile`；仅无有效 lease 的行；`lease_reconcile_e2e` + `multi_instance_reconcile_e2e` |
| DEP-01 | `[x]` | `attempt_id` 409/幂等；`deployment_conflict_e2e` |
| DEP-COMPOSE | `[x]` 产品路径 | `musehub-pipeline-oci` → `artifacts.oci_image_ref` 自动绑定 publish → API `compose-runner` 校验 `verified_source_oid`/`applied_oci_digest`；`verify-pipeline-oci-product-e2e.sh` 双 digest；`compose-runner-e2e` 仍为 runner 隔离探针 |
| OPS-01 | `[x]` | backup：`main` OID + artifact SHA（`verify-backup-restore.sh`） |
| IAM/PIPE/AGT | `[~]` | 生产 MCP：`m1_product_mcp` 部分工具（含 `musehub_pipeline_run` / `musehub_deployment_start` 写工具、仓库 list 游标）+ `mcp_idempotency_records` 写幂等；CI 矩阵 **Rust rmcp + 官方 Python `mcp` 2.3** 直连同一产品 `/mcp`（`production_mcp_matrix`，非 M0 probe 替代）；Streamable HTTP 会话 `mcp-session-id` PAT 绑定 + legacy session；V1 **PAT Bearer**（`remote_oauth_v1: false`）；Codex：`/adapters/codex/{status,task}` sim/可选 live（`online_verified` 无 CI 证据） |

## 未验证边界

- 不可信用户提交的 compose YAML（API **不接受**；仅 allowlist health URL）
- 多租户下跨节点 Runner 与网络策略（`-p` 项目名隔离 ≠ 完整租户隔离）
- 生产环境外部 Registry 凭据与受保护环境审批
