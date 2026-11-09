# MuseHub 首版进度（真实状态）

> 对照 `01-product-requirements.md` P0 与 README 验收主线。`[x]`=可真实操作；`[~]`=部分/探针/mock；`[ ]`=未实现。  
> 开发版本见根目录 `VERSION`。**不把 `m0_overall_passed` 或探针矩阵当作产品完备。**

## 本 PR 垂直切片目标

**验收脚本**：`./scripts/verify-product-acceptance.sh`（需 Postgres；含 up → product_slice → down → up）。

**可演示链路（Slice A）**：本地登录 → 创建组织内仓库（bare Git）→ `git clone/push`（HTTP Basic）→ 创建 Issue → 启动 **参考 Agent 适配器**（无模型）→ 产生可核对 Git 提交 → 取消运行无新提交。

## P0 清单

| ID | 状态 | 证据/限制 |
|---|---|---|
| IAM-01 | `[~]` | 登录/登出/会话；`principals`+`sessions`；无 CSRF/完整禁用用户全路径回归 |
| IAM-02 | `[~]` | `POST /api/v1/tokens`（PAT 仅展示一次）；Git Basic 支持 `mhp_*` 或密码；SSH 未做 |
| IAM-03 | `[ ]` | 仓库/API 授权基础 grant；跨组织负向测试不完整 |
| GIT-01 | `[~]` | POST 创建仓库 + `git init --bare`；`storage_key` 隔离路径 |
| GIT-02 | `[~]` | `git http-backend` 智能 HTTP；需配置 `MUSEHUB_GIT_ROOT` |
| GIT-03+ | `[ ]` | 浏览文件/历史/diff UI 未做 |
| ISS-01 | `[~]` | Issue 列表/创建 API+Web；无标签/负责人 |
| ISS-02 | `[~]` | `issue_revisions` + `implementation_plans`；接受计划后可跑 Agent；无 acceptance_criteria 表 |
| AGT-01 | `[~]` | **参考**适配器真实进程+提交；**Codex CLI 在线任务未验证**（见 ADR-008） |
| AGT-02 | `[~]` | 取消、用量字段；预算硬上限在凭证未知时不声称已达标 |
| PR/CI/REL/DEP/MCP-01/UI-01 | `[ ]` | 下一阶段 |

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
