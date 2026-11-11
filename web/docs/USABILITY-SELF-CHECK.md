# Usability self-check (Phase 1 UI split)

对照 README「首版验收主线」与「本地验证」中的真实用户路径，聚焦**可操作与可理解**，不以「表格全勾 / CI 全绿」替代本机体验。

## 路径与证据

| 步骤 | 证据（自动/脚本） | 本 Phase 1 控制台变化 | Mock / 边界 | 下一小缺口（不阻塞） |
|------|-------------------|------------------------|-------------|----------------------|
| 安装 / migration / 启动 | `deploy/RUNBOOK.md`、`product-stack` CI | 控制台改为 `/login` 深链；未改服务端 | 需本地 Postgres + migrate 手工步骤 | Mac 上 README 一键脚本与 Compose 卷路径一致性（文档级） |
| 登录 / 会话 | `product_slice`、`run-web-e2e.sh` | 独立 `/login`；会话在壳层 `session-handle` | bootstrap 密码仅首次 | 登录后默认落点 `/org/demo/repos` 需在文档写明 |
| 建仓库 | `00-product-flow` Playwright | `repo-list` 仅在仓库列表页 | 仍为 demo org | 仓库列表未展示空态文案 i18n |
| Issue → 计划 → Agent | 同上 e2e | `/org/:org/repos/:slug/issues` | reference agent，无模型 | 多 Issue 时滚动/聚焦未优化（Phase 2 UI） |
| PR / CI / Agent 修复 | `pr_agent_fix_http_e2e`、控制台 fixture | `/pulls` + 既有 testid | CI 面板依赖后端 job 状态 | 深链直达某一 PR# 未做（可用列表点选） |
| 流水线 / 部署 / 审批 | `pipeline_delivery_e2e`、deployment 控制台测试 | `/delivery` 单页承载原交付面板 | compose-mock / 策略开关见 UI 文案 | 壳层样式仍为 Phase 1 内联 CSS，非设计系统 |
| IAM / 审计 / Runner | HTTP e2e + IAM 切片 | 侧栏分路由：`/account/*`、`/org/*/audit` 等 | Org members 需 admin | 无 SSH 密钥独立页（后端未提供则不造假） |

## 本 PR 刻意未扩面

- Live Codex、TLS 双机、物理双 host、OAuth 暂缓项：仍按 PROGRESS / RUNBOOK 记载，**不作为**推迟 UI 信息架构拆分的理由。
- 不引入 shadcn/Chakra（见 `UI-PHASE2.md`）。

## 建议（仅记录，未实施）

| 主题 | 问题 | 收益 | 成本 | 替代 |
|------|------|------|------|------|
| 仓库上下文顶栏 | 仓库 tab 仅在侧栏且依赖 URL `slug` | 更像 Git hosting IA | 需 `RepoSubNav` 组件 + 测 | 维持侧栏二级链接（当前） |
| 默认 landing | 登录后进 repos 而非上次仓库 | 少一次点击 | 需 localStorage 策略 | 保持 repos 列表为 home |
