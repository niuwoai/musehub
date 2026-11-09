# MuseHub 首版进度（真实状态）

> 对照 `01-product-requirements.md` P0 与 README 验收主线。`[x]`=可真实操作；`[~]`=部分/探针/mock；`[ ]`=未实现。  
> 开发版本见根目录 `VERSION`。**不把探针或未跑 CI job 当作产品完备。**

## CI 门禁（须在 `develop` 上全绿）

| Job | 状态 | 证据 |
|-----|------|------|
| `rust-portable` | `[x]` | fmt/clippy/portable/acceptance/backup/安全 E2E |
| `web` | `[x]` | Playwright `scripts/run-web-e2e.sh`（失败时 `collect-e2e-diagnostics.sh`） |
| `compose-runner-e2e` | `[x]` | `scripts/verify-compose-runner-e2e.sh`（真实 pull/up/inspect/rollback，**非** `docker info` 探针） |

## P0 清单（节选）

| ID | 状态 | 证据/限制 |
|---|---|---|
| REC-01 | `[x]` | 启动 `boot_reconcile`；仅无有效 lease 的行；`lease_reconcile_e2e` + `multi_instance_reconcile_e2e` |
| DEP-01 | `[x]` | `attempt_id` 409/幂等；`deployment_conflict_e2e` |
| DEP-COMPOSE | `[x]` 产品路径 | `musehub-pipeline-oci` → `artifacts.oci_image_ref` 自动绑定 publish → API `compose-runner` 校验 `verified_source_oid`/`applied_oci_digest`；`verify-pipeline-oci-product-e2e.sh` 双 digest；`compose-runner-e2e` 仍为 runner 隔离探针 |
| OPS-01 | `[x]` | backup：`main` OID + artifact SHA（`verify-backup-restore.sh`） |
| IAM/PIPE/AGT | `[~]` | 见历史项；Codex 在线 / OCI 注册表 / 生产 MCP 仍未验证 |

## 未验证边界

- 不可信用户提交的 compose YAML（API **不接受**；仅 allowlist health URL）
- 多租户下跨节点 Runner 与网络策略（`-p` 项目名隔离 ≠ 完整租户隔离）
- 生产环境外部 Registry 凭据与受保护环境审批
