# M0 rmcp 互操作矩阵（双 SDK）

> 证据 scope：`m0_mcp_probe`（**不是**生产 MCP）｜开发版本见根目录 `VERSION`｜**M0 整体仍未通过**（`m0_overall_passed: false`）

## 开发计划对「两个客户端」的含义

`docs/05-development-plan.md` §4.5 与 §5「MCP 契约」层要求：**两个客户端实现**检查 rmcp/MCP 协议与 stdio、Streamable HTTP 接入；测试分层写明「Rust 客户端夹具 + **实际客户端**」。  
因此矩阵必须包含 **至少两种不同实现**，不能在同一 SDK 内仅改 `clientInfo.name` 冒充两个客户端。

本矩阵固定为：

| 客户端实现 | SDK / 版本 | 客户端名 | stdio | Streamable HTTP |
|---|---|---|---|---|
| `rust-rmcp` | crates.io `rmcp` **3.8.0** | `musehub-mcp-matrix-rust-rmcp` | `TokioChildProcess` → 探针 stdio 二进制 | `StreamableHttpClientTransport` (reqwest) + Bearer |
| `python-mcp-sdk` | PyPI `mcp` **2.3.0** | `musehub-mcp-matrix-python` | `StdioServerParameters` → 同一探针二进制 | `streamable_http_client` + `httpx2` Bearer |

共享探针服务端：`musehub-mcp-probe`（rmcp **3.8.0** 实现，非生产）。

## 复现

```sh
# 首次运行会通过 pip3 将 mcp==2.3.0 安装到 target/mcp-matrix-pydeps（需要网络，无付费 API）
rustup run stable cargo test -p musehub-mcp-probe --test matrix
```

Python 依赖锁定：`crates/musehub-mcp-probe/python/requirements.txt`。

## 覆盖的检查项

两种客户端在各自传输上均验证（若适用）：

- 协议协商（`protocol_version` / `peer_info`）
- `tools/list`
- `musehub_capabilities_get`（结构化信封 + `probe_scope`）
- `musehub_probe_echo` / 空参数拒绝
- `musehub_probe_error_sample`（工具级 `is_error` + 领域错误信封）
- HTTP：网关 **Bearer**；缺/错 token 不得完成握手（Rust + Python 各测一次）
- Python 额外：`unknown_tool_rejected`（Rust 用独立测试覆盖）
- Python Streamable HTTP 使用 `Client(..., mode="legacy")` 初始化；Rust 使用 `ClientLifecycleMode::Auto`——协商出的 `protocol_version` 以测试日志为准，可能同为 `2026-07-28` 或 `2025-11-25`。

## 机器可读摘要

[`m0-rmcp-matrix.json`](./m0-rmcp-matrix.json) 描述静态契约；`negotiated_protocol` 以 **`cargo test` 运行时**为准（JSON 中标注 `runtime: cargo test`）。

| 字段 | 含义 |
|---|---|
| `m0_mcp_interop_matrix_passed` | **本互操作切片**在自动化测试下是否成立（双 SDK × 双传输） |
| `m0_overall_passed` | 整个 M0 是否通过（仍为 **false**） |
| `write_idempotency_verified` | **false** — 未测写工具幂等 |

## 明确未覆盖

- 生产 `/mcp`、完整工具目录、领域授权、写工具 `idempotency_key` 重放
- OAuth / 桌面 MCP 宿主
- 第三方闭源客户端未列入本矩阵

## Git 标签说明

远程仓库存在 tag **`v0.1.0-rc2`**（由前一迭代推送，指向该次提交）。**本轮验收修正不视为发布**：当前工作区版本为 `0.1.0-rc3`（`VERSION`），**本轮不推送新 tag**（除非维护者另行决定发布流程）。

## 变更

- 2026-11-08：初版（仅双 rmcp 传输，已废止为不合格矩阵）
- 2026-11-08：增补官方 Python `mcp==2.3.0` 客户端，重构 JSON 与测试
