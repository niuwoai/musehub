# MuseHub 首版进度（真实状态）

> 对照 `01-product-requirements.md` P0 与 README 验收主线。`[x]`=可真实操作；`[~]`=部分/探针/mock；`[ ]`=未实现。  
> 开发版本见根目录 `VERSION`。**不把探针或未跑 CI job 当作产品完备。**

## CI 门禁（须在 `develop` 上全绿）

| Job | 状态 | 证据 |
|-----|------|------|
| `rust-portable` | `[x]` | fmt/clippy/portable/acceptance/backup/安全 E2E |
| `web` | `[x]` | Playwright `scripts/run-web-e2e.sh`（失败时 `collect-e2e-diagnostics.sh`）；`MUSEHUB_PRODUCT_OCI_E2E=1` 时 delivery 经 pipeline OCI 产品路径 |
| `compose-runner-e2e` | `[x]` | `scripts/verify-compose-runner-e2e.sh`（真实 pull/up/inspect/rollback，**非** `docker info` 探针） |
| `pipeline-oci-product` | `[x]` | rmcp 写 + Python **`mcp_product_oci_write_probe.py`（lite 在 e2e；full 在 verify 脚本）** + read probe |
| `product-stack` | `[x]` | `verify-product-stack-e2e.sh`（含 backup/upgrade/migrate） |
| `product-production-stack` | `[x]` | `verify-product-production-stack-e2e.sh`（remote worker 全链） |

## P0 清单（节选 — 领域）

| ID | 状态 | 证据/限制 |
|---|---|---|
| REC-01 | `[x]` | 启动 `boot_reconcile`；`lease_reconcile_e2e` + `multi_instance_reconcile_e2e` |
| DEP-01 | `[x]` | `attempt_id` 409/幂等；`deployment_conflict_e2e` |
| DEP-02 | `[x]` 控制台 | 受保护环境会话审批 + ID 加载；`deployment_approval_console` / `deployment-view-epoch` Playwright |
| DEP-02b | `[x]` 控制台 | 仓库范围列表：待审批发现 + **全部历史**分页（`offset`）、load more；`deployment_list_http_e2e` + `deployment-history-console` Playwright |
| DEP-03 | `[x]` 控制台 | 失败摘要 + **回滚**（`POST .../deployments/rollback`，会话写权限，**attempt_id** 幂等）；`deployment_rollback_e2e` + `deployment-rollback-console` Playwright（**fixture**，非 live runner 审批链） |
| DEP-COMPOSE | `[x]` 产品路径 | pipeline OCI → publish → compose-runner；`pipeline_oci_product_e2e` |
| OPS-01 | `[x]` | backup：`verify-backup-restore.sh`（含 Git OID） |
| IAM/PIPE/AGT | `[~]` | 生产 MCP **51 工具**（§5–9 `v1_implemented`）+ **5** 个 §13 资源读；side-effect 矩阵 E2E；`adapter=codex` **仅 live**（未配置 fail closed）；`adapter=codex-sim` 显式 sim |

## 首版缺口（源码 + 实测核查，诚实列表）

以下项在 README/01 首版叙事中常被提及，但当前仓库**未**达到可给真实租户用的完备度；按阻碍真实使用者排序。

| 主题 | 状态 | 现状 |
|------|------|------|
| 干净机器一键安装/启动 | `[x]` 脚本 | `install-production-compose.sh` + `configure-production-runner-pat.sh` + RUNBOOK；**systemd** 可选 `install-musehub-systemd.sh`（**未**在 CI/本仓库做真机 reboot 验收） |
| 生产静态 Web + 反代 TLS | `[~]` | 运营路径：`render-gateway-caddyfile.sh` + production compose TLS overlay（`manual`/`acme`）；`verify-caddyfile-layout.sh` 含 fixture 证布局；**未**验收真实 DNS/ACME/公网证书 |
| DB 迁移升级 Runbook | `[x]` 脚本 | `upgrade-production-compose.sh` + entrypoint migrate；host `verify-product-acceptance.sh` |
| HTTP Git `clone_url` 可达 | `[x]` 配置 | `MUSEHUB_PUBLIC_HTTP_ORIGIN`（默认 localhost 网关端口）；非 0.0.0.0 展示 |
| 最小 RBAC / PAT scope / 审计 | `[~]` | 控制台：成员/邀请、**自服务改密**（撤销其它会话）、PAT 一次展示+撤销；`iam_password_change_e2e` / `iam_org_privilege_e2e` / `iam_disabled_user_e2e`；**禁用为全局 principal**；**无** SSH key、**无**多租户 SSH 完备 |
| 安全 Git / 不可信贡献者 | `[~]` | forced-command 探针在 Colima；产品 Git HTTP + 分支保护有 E2E；**非**完整多租户 SSH 服务 |
| 独立 Runner 进程/服务 | `[x]` 本地 | `musehub-runnerd` + `run-ci-security-integration-local.sh`（remote pipeline/agent/deploy + cancel/kill/budget/epoch）；production stack；**非**物理双主机 |
| 部署回滚操作 | `[x]` 本地 | `deployment_rollback_runner_e2e` + production stack rollback/trace |
| 备份恢复 | `[x]` | `verify-backup-restore.sh` |
| 用户可编辑配置（非密钥） | `[~]` | `organizations.preferences` + PATCH settings（display_name/default_collaborator_role）；Web `config-grants.spec.ts` |
| 受保护环境部署审批 | `[x]` 本地 | `0027` + MCP `deployment_approve` 状态机；`protected_deployment_approval_e2e` |
| MCP V1 完整工具/Resources 读实现 | `[~]`（契约本地可停扩） | **22** 写幂等均有 MCP 副作用/门禁/幂等证明（`writes_thin` 含 `agent_run_start`；OCI 链 `pipeline_run` replay）；§13 catalog + negatives；**无** Live Codex/OAuth |
| Codex live 在线 | `[~]` | `adapter=codex` 仅 live；`codex-sim` 显式；**无** CI 在线凭据（刻意） |

## 未验证边界

- 不可信用户提交的 compose YAML（API **不接受**；仅 allowlist health URL）
- 多租户下跨节点 Runner 与网络策略
- 生产环境外部 Registry 凭据；受保护环境审批已在本地 VM 验（非物理双机）
