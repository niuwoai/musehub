# 本地验收跑批记录

> Branch: `cursor/local-product-integration-5f97` → PR [#16](https://github.com/niuwoai/musehub/pull/16)  
> **本 commit SHA**: `e86d942`
> **未** 触发 GitHub Actions merge/tag。

## 本地可交付（VM 已跑通）

| 类别 | Exit | 证据 / 日志 |
|------|------|-------------|
| 生产 compose 全链 | 0 | `verify-product-production-stack-e2e.sh` |
| CI security 镜像（本地） | 0 | `run-ci-security-integration-local.sh`（含 plan steps、protected approval、side-effects、§5–9 合约） |
| Playwright（含 audit/runner） | 0 | `run-web-e2e.sh` **6 passed** |
| 迁移 0026/0027 schema | 0 | `verify-product-acceptance.sh` → `schema_check=ok` |

## 本迭代契约补强

| 项 | 状态 |
|---|---|
| `musehub_plan_create` | `steps[]` 校验（PlanStep）、JSONB 落库、`plan_get` 回读 |
| 受保护环境 | `0027_protected_deployment`；`awaiting_approval` → `deployment_approve` → 本地 compose / 远程 `queued`；审计 + 自审拒绝 + 跨租户拒绝 |
| E2E | `production_mcp_plan_steps_e2e`、`protected_deployment_approval_e2e`、`production_mcp_contract_side_effects_e2e` |

## 诚实外部缺口

| 主题 | 说明 |
|------|------|
| Live Codex 在线 | 需用户凭据 |
| 物理双主机拓扑证据 | **独立 Linux Runner 仍为 V1**（`01` CI-02、`README`）；双机用跨容器代替（`ACCEPTANCE-EVIDENCE.md`） |
| OAuth MCP 远程 | `04-mcp-api.md` 首版未承诺；`oauth_verified=false` |
