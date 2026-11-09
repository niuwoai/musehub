# ADR-008: M0 Agent 适配器选型

**状态**：已采纳（M0 切片）  
**日期**：2026-11-09

## 决策

1. **生产目标（待用户凭证验证）**：OpenAI **Codex CLI**（`codex exec` 等 headless 子命令）。理由：官方 CLI、可在隔离工作目录运行、支持结构化/JSON 输出方向、与「受控子进程适配器」架构一致（见 `02-architecture.md` §7）。
2. **本仓库已验证路径**：`musehub-agent-reference` 二进制（`adapter_kind=reference`）。确定性、无模型、产生真实 Git 提交；**不构成 AGT-01「真实模型 Agent」验收**。
3. **预算**：适配器必须上报 `usage` 事件；缺失时 `budget_unknown=true`，平台不声称已验证费用硬上限。

## 未验证 / 阻塞

| 项 | 说明 |
|---|---|
| Codex 在线任务 | 需用户提供 API/账户与 `MUSEHUB_AGENT_CODEX_BIN`；本 Agent **未使用** Cloud 运行环境既有密钥 |
| 取消 | 参考适配器支持 `start_kill`；Codex 取消语义需在凭证可用后补互操作测试 |
| 第二适配器 | ADR-006 仍要求 eventual 第二实现；不在本切片范围 |

## 后果

- CI 与默认可复现证据以 **reference** 为准。
- 文档与 API 响应必须标注 `adapter_kind`，禁止将 reference 记为「生产 Agent 已选型完成」。
