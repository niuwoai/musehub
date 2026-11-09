# 本地验收跑批记录

> Branch: `cursor/local-product-integration-5f97` → PR [#16](https://github.com/niuwoai/musehub/pull/16)  
> **本 commit SHA**: `e178d24ddab55dce7d367f02edbb8578209954ac`
> **未** 触发 GitHub Actions merge/tag。

## 本地可交付（VM 已跑通）

| 类别 | Exit | 证据 / 日志 |
|------|------|-------------|
| 生产 compose 全链 | 0 | `verify-product-production-stack-e2e.sh` |
| CI security 镜像（本地） | 0 | `run-ci-security-integration-local.sh`（含 MCP §5–9 合约读/守卫、write expansion） |
| Playwright（含 audit/runner） | 0 | `run-web-e2e.sh` **6 passed** |
| 迁移 0026 schema | 0 | `verify-product-acceptance.sh` → `schema_check=ok` |

## MCP `docs/04-mcp-api.md` §5–9

| 项 | 状态 |
|---|---|
| 工具目录 §5–9 | **51** 个 `implemented_tools`（含 `deployment_trace`）；`maturity=v1_implemented`，`full_contract_tools=true` |
| 迁移 | `0026_mcp_contract`（criteria、discussion、agent messages、job logs、deployment approvals） |
| E2E | `production_mcp_contract_reads_e2e`、`production_mcp_contract_guards_e2e`、`production_mcp_write_expansion_e2e` |

## 诚实外部缺口

| 主题 | 说明 |
|------|------|
| Live Codex 在线 | 需用户凭据 |
| 物理双主机拓扑证据 | **独立 Linux Runner 仍为 V1**（`01` CI-02、`README`）；双机用跨容器代替（`ACCEPTANCE-EVIDENCE.md`） |
| OAuth MCP 远程 | `04-mcp-api.md` 首版未承诺；`oauth_verified=false` |
