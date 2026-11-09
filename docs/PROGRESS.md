# MuseHub 首版进度（真实状态）

> 对照 `01-product-requirements.md` P0 与 README 验收主线。`[x]`=可真实操作；`[~]`=部分/探针/mock；`[ ]`=未实现。  
> 开发版本见根目录 `VERSION`。**不把探针或未跑 CI job 当作产品完备。**

## CI 门禁（须在 `develop` 上全绿）

| Job | 状态 | 证据 |
|-----|------|------|
| `rust-portable` | `[x]` | fmt/clippy/portable/acceptance/backup/安全 E2E |
| `web` | `[x]` | Playwright `scripts/run-web-e2e.sh`（失败时 `collect-e2e-diagnostics.sh`）；`MUSEHUB_PRODUCT_OCI_E2E=1` 时 delivery 经 pipeline OCI 产品路径 |
| `compose-runner-e2e` | `[x]` | `scripts/verify-compose-runner-e2e.sh`（真实 pull/up/inspect/rollback，**非** `docker info` 探针） |
| `pipeline-oci-product` | `[x]` | `verify-pipeline-oci-product-e2e.sh`：双 digest + rmcp/Python MCP 写/trace；PR #12+ |

## P0 清单（节选 — 领域）

| ID | 状态 | 证据/限制 |
|---|---|---|
| REC-01 | `[x]` | 启动 `boot_reconcile`；`lease_reconcile_e2e` + `multi_instance_reconcile_e2e` |
| DEP-01 | `[x]` | `attempt_id` 409/幂等；`deployment_conflict_e2e` |
| DEP-COMPOSE | `[x]` 产品路径 | pipeline OCI → publish → compose-runner；`pipeline_oci_product_e2e` |
| OPS-01 | `[x]` | backup：`verify-backup-restore.sh`（含 Git OID） |
| IAM/PIPE/AGT | `[~]` | 生产 MCP partial（19 工具 + §13 资源模板）；Codex **主线**可选 `adapter=codex`（live bin 或 sim）；fixture 仅 CI |

## 首版缺口（源码 + 实测核查，诚实列表）

以下项在 README/01 首版叙事中常被提及，但当前仓库**未**达到可给真实租户用的完备度；按阻碍真实使用者排序。

| 主题 | 状态 | 现状 |
|------|------|------|
| 干净机器一键安装/启动 | `[ ]` | 有 `.env.example` 与 migrate 命令；**无** `deploy/`、systemd、健康依赖编排文档化安装包 |
| 生产静态 Web + 反代 TLS | `[ ]` | `yarn build` 有；**无** 捆绑静态服务、Caddy/nginx 示例、证书轮转 |
| DB 迁移升级 Runbook | `[~]` | `musehub-migrate up/down` + CI migrate；**无** 多版本滚动升级验收 |
| 最小 RBAC / PAT scope / 审计 | `[~]` | PAT + org 成员 + `pat_scope`；**无** `audit_events` 表、`musehub_audit_list`、细粒度 repo grant |
| 安全 Git / 不可信贡献者 | `[~]` | forced-command 探针在 Colima；产品 Git HTTP + 分支保护有 E2E；**非**完整多租户 SSH 服务 |
| 独立 Runner 进程/服务 | `[~]` | 进程内 supervisor + OCI/compose 子进程；**无** 远端 worker 注册与隔离策略产品化 |
| 部署回滚操作 | `[~]` | compose-runner 探针可 rollback；产品 API **未**暴露一键 rollback MCP/REST |
| 备份恢复 | `[x]` | `verify-backup-restore.sh` |
| 用户可编辑配置（非密钥） | `[~]` | 环境变量为主；**无** 管理 UI 配置面 |
| MCP V1 完整工具/Resources 读实现 | `[~]` | 19 工具 + 5 资源模板；issue/PR/commit file **读资源未实现**（capabilities 标明 partial） |
| Codex live 在线 | `[~]` | `MUSEHUB_AGENT_CODEX_BIN` 主线 spawn；**无** CI 在线凭据（刻意）；probe 与 task 仍分离门卫 |

## 未验证边界

- 不可信用户提交的 compose YAML（API **不接受**；仅 allowlist health URL）
- 多租户下跨节点 Runner 与网络策略
- 生产环境外部 Registry 凭据与受保护环境审批
