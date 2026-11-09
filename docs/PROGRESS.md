# MuseHub 首版进度（真实状态）

> 对照 `01-product-requirements.md` P0 与 README 验收主线。`[x]`=可真实操作；`[~]`=部分/探针/mock；`[ ]`=未实现。  
> 开发版本见根目录 `VERSION`。**不把 `m0_overall_passed` 或探针矩阵当作产品完备。**

## 本 PR 垂直切片目标

**验收脚本**：`./scripts/verify-product-acceptance.sh`（需 Postgres；含 up → product_slice → down → up）。

**可演示链路（Slice A）**：本地登录 → 创建组织内仓库（bare Git）→ `git clone/push`（HTTP Basic，**default 禁直推**）→ Issue + **接受计划**（绑定 `current_revision`）→ 异步 **参考 Agent** → `GET /agent-runs/{id}` / **取消杀进程组** → 创建 **PR / 审查 / 合并**（禁自审、非空 diff）→ `POST /api/v1/adapters/codex/probe`（**sim 协议**，无在线证据）。

## P0 清单

| ID | 状态 | 证据/限制 |
|---|---|---|
| IAM-01 | `[~]` | 登录/登出/会话；`principals`+`sessions`；无 CSRF/完整禁用用户全路径回归 |
| IAM-02 | `[~]` | `POST /api/v1/tokens`（PAT 仅展示一次）；Git Basic 支持 `mhp_*` 或密码；SSH 未做 |
| IAM-03 | `[~]` | `auth_matrix`：非成员 403、outsider PAT Git 403 |
| GIT-01 | `[~]` | POST 创建仓库 + `git init --bare`；`storage_key` 隔离路径 |
| GIT-02 | `[~]` | `git http-backend` + pre-receive **保护 default**；`agent/*` 允许 |
| GIT-03+ | `[ ]` | 浏览文件/历史/diff UI 未做 |
| ISS-01 | `[~]` | Issue 列表/创建 API+Web；无标签/负责人 |
| ISS-02 | `[~]` | `issue_revisions` + `implementation_plans`；接受计划后可跑 Agent；无 acceptance_criteria 表 |
| AGT-01 | `[~]` | **参考**适配器真实进程+提交；**Codex CLI 在线任务未验证**（见 ADR-008） |
| AGT-02 | `[~]` | `agent_cancel_e2e`：取消后无 `result_commit_oid`；进程组 SIGKILL |
| PR-01 | `[~]` | REST：checks/head CAS/sync + `pr_gates_e2e`/`pr_security_e2e`；Web 列表/创建 PR；合并经 fetch+CAS update-ref（pre-receive 常开） |
| PIPE-01 | `[~]` | 参考 pipeline/制品本地存储/Release/compose-mock 部署/trace；`pipeline_delivery_e2e`（stacked PR，依赖 PR#3） |
| AGT-02 | `[~]` | `budget_tokens` 接线 + `agent_budget`；取消竞态写前检查 |
| IAM-02 | `[~]` | PAT revoke/scope/org + `pat_scope` |
| CI/REL/DEP/MCP-01/UI-01 | `[ ]` | 流水线/制品/Compose/追溯、业务 MCP、PR Web |

## 探针（非产品）

| 组件 | 说明 |
|---|---|
| `musehub-*-probe` | M0 风险验证；Colima/Homebrew 见 `.github/workflows/evidence-macos.yml` |
| `musehub-mcp-probe` | 互操作矩阵；非生产 `/mcp` |

## 本地启动（Slice A）

```sh
export DATABASE_URL=postgres://...
export MUSEHUB_GIT_ROOT=/tmp/musehub-git-data
export MUSEHUB_SESSION_SECRET=$(openssl rand -hex 32)
# 可选一次性引导：MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=...
musehub-migrate up   # 需实现 CLI 或 cargo run -p musehub-migrate -- up
cargo run -p musehub-server
yarn --cwd web dev    # 代理 /api
```

详见 README「本地验证」更新段。
