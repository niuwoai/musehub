# M0 rmcp 互操作矩阵

> 证据类型：`m0_mcp_probe`（**不是**生产 MCP 服务）｜rmcp crate **3.8.0**（锁定）｜开发版本 `0.1.0-rc2`｜**M0 整体仍未通过**

## 范围

| 在范围内 | 不在范围内 |
|---|---|
| stdio 传输 + `TokioChildProcess` 客户端 | 生产 `musehub-server` `/mcp` 挂载 |
| Streamable HTTP + `StreamableHttpClientTransport`（reqwest） | OAuth / 第三方 MCP 桌面客户端实测 |
| 协议协商（`2025-11-25` / `2026-07-28` 候选） | 写工具 `idempotency_key` 幂等重放 |
| 工具发现、`musehub_capabilities_get` / `musehub_probe_echo` / `musehub_probe_error_sample` | 完整 50 工具目录（MCP-IMP-02） |
| MuseHub 结构化信封（`schema_version` + `request_id` + `data`/`error`） | 声明 M0 已通过 |

两个客户端实现名称固定为：

- stdio：`musehub-mcp-matrix-stdio-client`
- HTTP：`musehub-mcp-matrix-http-client`

探针服务端标识：`musehub-mcp-probe`，scope 常量 `m0_mcp_probe`。

## 复现命令

在仓库根目录（Rust stable ≥ 1.88，网络仅用于首次 `cargo` 依赖下载，**无模型/外部 API 费用**）：

```sh
rustup run stable cargo test -p musehub-mcp-probe --test matrix
rustup run stable cargo test -p musehub-mcp-probe
```

手动启动 stdio 探针（供外部 MCP 客户端试用，非矩阵必需）：

```sh
rustup run stable cargo run -p musehub-mcp-probe --bin musehub-mcp-probe-stdio
```

HTTP 探针 Bearer 由环境变量 `MUSEHUB_MCP_PROBE_BEARER` 配置（仅集成测试内置随机/固定值；生产策略未定义）。

## 机器可读摘要

[`m0-rmcp-matrix.json`](./m0-rmcp-matrix.json) 与 `matrix.rs` 中的 `MatrixReport` 结构对齐。`negotiated_protocol` 字段在 JSON 中保留占位符；**实测协商版本以测试运行时 `peer_info` 为准**（随 rmcp 与客户端 lifecycle 可能为上表候选之一）。

## 检查项

| 传输 | 客户端 | 检查 |
|---|---|---|
| stdio | 子进程 + rmcp client | initialize、list_tools、capabilities、echo、空参数拒绝、error_sample 信封 |
| streamable_http | reqwest transport | 同上 + 缺/错 Bearer 在 MCP 握手前返回 401 |

## 未验证 / 失败声明

- **写操作幂等**：`write_idempotency_verified: false`（无带 `idempotency_key` 的写工具）。
- **OAuth MCP**：未实现、未测试。
- **外部 MCP 客户端**（Cursor、Claude Desktop 等）：未列入本矩阵；需另行记录。
- **`m0_overall_passed`：false** — Agent、多租户、竞品试用等 M0 项仍开放。

## 变更记录

- 2026-11-08：初版矩阵与 `musehub-mcp-probe` crate 落地。
