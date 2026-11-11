# Usability self-check (Phase 1 UI split)

对照 README「首版验收主线」与「本地验证」中的真实用户路径，聚焦**可操作与可理解**，不以「表格全勾 / CI 全绿」替代本机体验。

## 路径与证据

| 步骤 | 证据（自动/脚本） | 本 Phase 1 控制台变化 | Mock / 边界 | 下一小缺口（不阻塞） |
|------|-------------------|------------------------|-------------|----------------------|
| 安装 / migration / 启动 | `deploy/RUNBOOK.md`、`product-stack` CI | `/login` 深链；`session-boot` 恢复中不误踢回登录 | 需本地 Postgres + migrate 手工步骤 | Mac 上 README 一键脚本与 Compose 卷路径一致性（文档级） |
| 登录 / 会话 | `session-routing.spec.ts`、`00-product-flow` | 未登录深链 → 登录后 `from` 回跳；刷新深链保留 | bootstrap 密码仅首次 | 浏览器后退栈未单独测（可用手动点验） |
| 建仓库 | Playwright `createRepoViaUi` | 创建后进入 `/pulls` | 仍为 demo org | — |
| Issue → 计划 → Agent | `00-product-flow` | `/issues` | reference agent | 多 Issue 时滚动（Phase 2） |
| PR / CI / Agent 修复 | `delivery-flow`、`pr_agent_fix_*` | `/pulls`（创建 PR 后自动导航） | 依赖后端 job 状态 | PR# 深链未做 |
| 流水线 / 部署 | `delivery-flow`、deployment/pipeline console specs | `/delivery` | compose-mock 文案 | 设计系统延后 Phase 2 |
| IAM | `iam-*`、`audit-runner` | 侧栏分路由 | Org members 需 admin | 无假 SSH 页 |

## 本 PR 刻意未扩面

- Live Codex、TLS 双机、OAuth 暂缓项不以此阻塞 UI IA。
- 不引入 shadcn/Chakra（见 `UI-PHASE2.md`）。

## 生产 SPA 回退

Compose/Caddy：`deploy/Caddyfile`、`deploy/Caddyfile.ci` 已含 `try_files {path} /index.html`；与 Vite dev 深链行为对齐（见 README「Web 控制台」）。
