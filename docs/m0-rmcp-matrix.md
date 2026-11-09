# M0 rmcp 互操作矩阵（双 SDK）

> 证据 scope：`m0_mcp_probe`（**不是**生产 MCP）｜开发版本见根目录 `VERSION`｜**M0 整体仍未通过**（`m0_overall_passed: false`）

## 测试分层（必读）

| 类型 | 位置 | 目的 | 本 PR 数量（`cargo test -p musehub-mcp-probe`） |
|---|---|---|---|
| **单元测试** | `src/lib.rs` (`mod tests`) | 信封、Bearer 解析、能力负载、输入边界；不启网络/子进程 | **8** |
| **HTTP 门控** | `tests/http_bearer.rs` | Axum 路由：/healthz 开放、/mcp 必 Bearer；不走 MCP | 3 |
| **探针级 E2E mock** | `tests/e2e_probe.rs` | 真实 stdio 子进程 / 临时 HTTP 端口完整收发与负向；**不是** MuseHub 平台 E2E | 3 |
| **协议互操作矩阵** | `tests/matrix.rs` | 双 SDK × 双传输 + Bearer 负例；Rust `rmcp` + Python `mcp` | 9 |
| **Python 管线** | `tests/python_harness.rs` + `python/mcp_matrix_runner.py` | 独立 Python 客户端实现 | （被 matrix 调用） |

说明：8 项矩阵 **不等于**「全仓库单元 + 业务 E2E 已完成」。平台级账号/UI/数据库 E2E **未实现、未声称**。

## 开发计划对「两个客户端」的含义

`docs/05-development-plan.md` §4.5 与 §5：至少 **两种不同客户端实现**（非同一 SDK 改名）。

| 客户端实现 | SDK / 版本 | 客户端名 | stdio | Streamable HTTP |
|---|---|---|---|---|
| `rust-rmcp` | `rmcp` **3.8.0** | `musehub-mcp-matrix-rust-rmcp` | `TokioChildProcess` | `StreamableHttpClientTransport` + Bearer |
| `python-mcp-sdk` | `mcp` **2.3.0** | `musehub-mcp-matrix-python` | `StdioServerParameters` | `streamable_http_client` + `httpx2` Bearer |

Python 连接模式（与代码一致，见 `mcp_matrix_runner.py` 常量）：

- stdio：`Client(..., mode="2026-07-28")`
- HTTP：`Client(..., mode="legacy")`（与 rmcp 探针 Streamable HTTP 会话兼容）
- HTTP Bearer 负例探测：同样 `legacy`

Rust HTTP：`ClientLifecycleMode::Auto`（优先 `2026-07-28`，回退 `2025-11-25`）。

## 复现

```sh
# 1) 一次性准备 Python 依赖（推荐；测试默认不再动态 pip）
chmod +x ./scripts/setup-mcp-matrix-python-deps.sh
./scripts/setup-mcp-matrix-python-deps.sh

# 2) 运行全部探针测试
rustup run stable cargo test -p musehub-mcp-probe -- --test-threads=1
```

无预装且未设置 `MUSEHUB_MCP_MATRIX_ALLOW_PIP=1` 时，Python 相关用例 **诚实失败** 并提示运行 setup 脚本（不 `#[ignore]` 冒充通过）。

应急（需网络）：`MUSEHUB_MCP_MATRIX_ALLOW_PIP=1 cargo test -p musehub-mcp-probe --test matrix`

### 本环境实跑（2026-11-09）

| 命令 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy -p musehub-mcp-probe --all-targets -- -D warnings` | 通过 |
| `cargo test -p musehub-mcp-probe -- --test-threads=1` | **23** 通过（8 单元 + 3 Bearer + 3 E2E + 9 矩阵） |
| `yarn --cwd web test` | 1 通过 |
| `yarn --cwd web build` | 通过 |

前置：`./scripts/setup-mcp-matrix-python-deps.sh`（`target/mcp-matrix-pydeps`）。

## 机器可读摘要

[`m0-rmcp-matrix.json`](./m0-rmcp-matrix.json) 为**静态契约**（工具名、检查项、客户端模式）。`negotiated_protocol` 在 JSON 中为 `null` —— **禁止**把未实测的版本写死；真实协商结果仅存在于测试运行时。

## 安全与边界（自审要点）

- Bearer 仅存于测试常量 / 请求头；**未**写入结构化信封或日志字段。
- stdio 探针二进制 **不写** stdout（仅 MCP JSON-RPC）；Python runner **仅**向 stdout 输出一行 JSON 报告（测试解析），错误走 stderr。
- HTTP：错误 Bearer 在 MCP 握手前返回 **401**（见 `http_bearer` + matrix）。
- 写工具幂等：**未验证**（`write_idempotency_verified: false`）。

## Git 标签

远程存在历史 tag `v0.1.0-rc2`（先前迭代推送）。当前工作区版本见 `VERSION`；**本轮不推送新 tag、不发布**。

## 变更

- 2026-11-09：测试分层、Python 离线 setup、静态 JSON 去伪 negotiated_protocol、Bearer/ E2E 增补
