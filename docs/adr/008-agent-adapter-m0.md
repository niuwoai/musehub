# ADR-008: M0 Agent 适配器选型

**状态**：已采纳（M0 切片）  
**日期**：2026-11-09

## 决策

1. **生产目标（待用户凭证验证）**：OpenAI **Codex CLI**（`codex exec` 等 headless 子命令）。理由：官方 CLI、可在隔离工作目录运行、支持结构化/JSON 输出方向、与「受控子进程适配器」架构一致（见 `02-architecture.md` §7）。
2. **本仓库已验证路径**：`musehub-agent-reference` 二进制（`adapter_kind=reference`）。确定性、无模型、产生真实 Git 提交；**不构成 AGT-01「真实模型 Agent」验收**。
3. **预算**：token 与金额用量分别跟踪；缺失时对应 `*_unknown=true`。Codex CLI 可上报 token usage，但不提供本适配器可用的金额 usage；配置金额预算时必须 fail closed。

## 未验证 / 阻塞

| 项 | 说明 |
|---|---|
| Codex 在线任务 | `codex exec --json` JSONL 与 token usage 接入已实现；`scripts/verify-live-codex-agent-e2e.sh` 提供显式 opt-in 的 Runner 端到端入口。仍需用户提供 CLI/账户、token 预算并实际运行；本次未调用模型，故真实在线任务与预算证据仍未验收 |
| 取消 | 参考适配器支持 `start_kill`；Codex 取消语义需在凭证可用后补互操作测试 |
| 第二适配器 | ADR-006 仍要求 eventual 第二实现；不在本切片范围 |

## 后果

- CI 与默认可复现证据以 **reference** 为准；live Codex acceptance 不接入普通 CI。
- Codex 的每个完成 turn 提供的 token usage 用于预算拦截；无 usage 的 run 不满足显式 token budget。Codex 不报告金额用量，显式金额预算会 fail closed。
- Git-backed AgentRun 只有在原工作分支产生新 commit 且 turn 正常完成时才返回成功；没有初始 Git commit 的临时任务工作区允许无 commit 完成。
- Codex 子进程只继承运行所需的基础环境变量，不继承 Runner PAT、数据库 URL 或服务端密钥。
- 文档与 API 响应必须标注 `adapter_kind`，禁止将 reference 记为「生产 Agent 已选型完成」。
