# 本地验收跑批记录

> Branch: `cursor/local-product-integration-5f97` → PR [#16](https://github.com/niuwoai/musehub/pull/16)  
> **本 commit SHA**: `14abc09`（`run-ci-security-integration-local.sh=ok`，`run-web-e2e.sh` 6 passed）  
> **未** 触发 GitHub Actions merge/tag；用户可在本仓库联跑更新后的 `ci.yml`。

## 本地可交付（VM 已跑通）

| 类别 | Exit | 证据 / 日志 |
|------|------|-------------|
| 生产 compose 全链 | 0 | `verify-product-production-stack-e2e.sh` → `product_production_stack_e2e=ok` |
| CI security 镜像（本地） | 0 | `run-ci-security-integration-local.sh`（含 remote deploy/agent cancel/kill/budget/epoch） |
| OCI 产品路径 | 0 | `sg docker ./scripts/verify-pipeline-oci-product-e2e.sh` |
| Demo stack + backup/upgrade | 0 | `verify-product-stack-e2e.sh` |
| Runbook 干净安装 | 0 | `verify-clean-install-runbook.sh` → `clean_install_runbook=ok` |
| 备份恢复 | 0 | `verify-backup-restore.sh` → `backup_restore_verify=ok` |
| 生产 profile fail-closed | 0 | `verify-product-production-profile.sh` |
| Playwright（含 audit/runner） | 0 | `run-web-e2e.sh` 6 passed（`e2e/audit-runner.spec.ts`） |
| 子进程 remote | 0 | `verify-remote-deployment-e2e.sh`、`verify-remote-agent-e2e.sh` |

## 与用户联跑 CI（仓库已同步，未 push 触发）

- `rust-portable` 现调用 **`./scripts/run-ci-security-integration-local.sh`**（与本地矩阵一致，含 remote 全套）。
- 新增 job **`product-production-stack`** → `verify-product-production-stack-e2e.sh`。

## 诚实缺口（非“外部阻塞”但与 04-mcp-api 全集差距）

| 主题 | 状态 | 说明 |
|------|------|------|
| MCP 工具全集 | `[~]` | 已实现 **26** 工具 + **5** 资源读（含 `repository_create`、`issue_update`、`pr_create`/`pr_list`/`pr_merge`）；discuss/plan 扩展读等仍缺 |
| Web 信息架构 | `[~]` | 单页主线 + **只读**审计/Runner 状态面板（`audit-panel`、`runner-status-panel`）；无完整管理台 |
| systemd / 多机 HA | 外部 | V1 明确排除 |
| Live Codex 在线 | 外部 | 需凭据；`codex-sim` / fixture 已验 |
| 物理双主机拓扑证据 | 外部 | **独立 Linux Runner 仍为 V1**（`01` CI-02、`README`）；双机证据用跨容器代替，见 `ACCEPTANCE-EVIDENCE.md` |

## 新增脚本

| 脚本 | 用途 |
|------|------|
| `scripts/install-production-compose.sh` | 生成 `deploy/.env`、构建并启动 production 控制面 |
| `scripts/verify-clean-install-runbook.sh` | 实测 `deploy/RUNBOOK.md` quick start |
