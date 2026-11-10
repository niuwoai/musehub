# 本地验收跑批记录

> Branch: `cursor/local-product-integration-5f97` → PR [#16](https://github.com/niuwoai/musehub/pull/16)  
> **本 commit SHA**: `26dd933`
> **未** 触发 GitHub Actions merge/tag。

## 本地可交付（VM 已跑通）

| 类别 | Exit | 证据 / 日志 |
|------|------|-------------|
| 生产 compose 全链 | 0 | `verify-product-production-stack-e2e.sh`（前序回合） |
| CI security 镜像（本地） | 0 | `run-ci-security-integration-local.sh`（含 `writes_thin` / `writes_remainder` / `resources_v1_*`） |
| Playwright（含 audit/runner） | 0 | `run-web-e2e.sh` **6 passed** |
| 迁移 0026/0027 schema | 0 | `verify-product-acceptance.sh` → migrate 往返 + `verify-caddyfile-layout.sh`（Docker 下 `caddy validate` 真验） |

## MCP 契约（§5–9 / §13）

| 项 | 状态 |
|---|---|
| 工具目录 | **51** `implemented_tools`；`maturity=v1_implemented` |
| 写 side-effect 矩阵 | `production_mcp_contract_side_effects_e2e`（7 条：discussion、criteria、plan、release、resolve、幂等冲突等） |
| 写工具余量矩阵 | `writes_remainder`（5）+ `writes_thin`（**6**）：含 `agent_run_start` 门禁/幂等；`pipeline_oci_product_e2e` MCP `pipeline_run` **replay** |
| plan / 受保护部署 | `production_mcp_plan_steps_e2e`、`protected_deployment_approval_e2e` |
| §13 资源读 | **5** URI 与 capabilities 一致；`resources_v1_catalog`（含 **跨 org** PAT 拒绝 4 URI）+ `resources_v1_negatives`（selected 跨仓） |

## 本地 V1 契约（停扩判断）

`WRITE_IDEMPOTENCY_TOOLS` **22/22** 均已有 MCP 矩阵或专用 E2E（副作用 / 门禁负例 / 幂等 replay 或冲突）。除下表外部项外，**本地 VM 无明确 V1 契约缺口**；后续宜等用户联跑 CI 或提供 Live Codex 凭据再扩。

## 诚实外部缺口

| 主题 | 说明 |
|------|------|
| Live Codex 在线 | 需用户凭据；fail-closed |
| 物理双主机拓扑证据 | 跨容器 remote runner 代替；**非**双人真机 |
| OAuth MCP 远程 | 首版未承诺 |
| GitHub Actions | 本分支刻意不触发 |
