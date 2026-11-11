# Changelog

## [Unreleased]

### Fixed

- Migration `0005_fk_indexes` for BOOT-02 schema check; Git HTTP route fix (`/git/{org}/{repo}.git/...`).
- Bootstrap 强密码策略；Agent 分支 `agent/*` 校验；Git 路径穿越拒绝。
- `verify-product-acceptance.sh`；CI 强 bootstrap 密码与 reference 二进制预构建。
- Web：`deployment-history-console` 等部署列表切换到「全部历史」与 `deployment-rollback-console` 共用 `switchDeploymentListToAll`，修复 main 上列表断言超时（merge `ef356679` 回归）。

### Added

- **S1 协作切片（Web + REST）**：Code 根树浏览 + git 输出上限；Issue 列表筛选/游标 UI、标签与组织内负责人校验；PR diff **行锚点校验**、`expected_row_version` sync CAS、outdated 与 sync 事务一致；SSH **活跃指纹唯一**（`0030`，撤销后可重加）；控制台 en/zh（**不含**产品级 Git-over-SSH 守护进程）。
- `s1_collaboration_http_e2e`、`web/e2e/s1-collaboration.spec.ts`。
- 产品垂直切片：`musehub-app`（登录/会话、bare 仓库、Git smart HTTP、Issue、**reference** Agent 运行）。
- `musehub-agent-adapter` + ADR-008；`docs/PROGRESS.md`；migrations `0003`/`0004`。
- GitHub `ci.yml`（可移植测试 + Postgres 集成）与 `evidence-macos.yml`（Colima 证据，非门禁）。
- `./scripts/dev-local-stack.sh`：本地 loopback `init` / `start` / `status` / `stop`（`MUSEHUB_BIND` / `MUSEHUB_DEV_API_URL` 一致、`pg_isready`、进程 `started_epoch` 认领、失败仅回收本次子进程）；`verify-dev-local-stack*.sh` 正/负例。Linux CI 已验；**macOS 未在 Agent 环境实测**。
- `scripts/lib/sed-portable.sh`：macOS/Linux `sed_inplace`；生产安装与 runbook 验证复用。

## [0.1.0-rc4]

### Fixed

- MCP 测试：默认禁止测试进程内 `pip`；新增 `scripts/setup-mcp-matrix-python-deps.sh`。
- 静态 `m0-rmcp-matrix.json` 不再伪造 `negotiated_protocol`；Python Bearer 负例与 HTTP 一致使用 `legacy` 模式。
- 增补 Bearer 门控、探针级 E2E mock 与 lib 单元测试。

## [0.1.0-rc3]

开发中版本，不是产品发布（`v0.1.0-rc2` Git 标签为历史推送，不代表本轮发布）。

### Fixed

- MCP 矩阵：改用 **rust-rmcp** 与 **Python mcp==2.3.0** 两种独立 SDK（禁止同 SDK 仅改 client 名称冒充双客户端）；Python HTTP 在 `spawn_blocking` 下运行以免阻塞 Tokio 探针服务。

### Added

- `crates/musehub-mcp-probe/python/`：`mcp_matrix_runner.py` + 锁定 `requirements.txt`。

## [0.1.0-rc2]

### Added

- 新增 `musehub-mcp-probe`：基于 rmcp `3.8.0` 的 M0 stdio + Streamable HTTP 探针（初版矩阵口径不足，见 rc3 修正）。

## [0.1.0-rc1]

### Added

- 建立 Rust workspace：`musehub-core`、`musehub-server`、`musehub-git-probe`。
- 以根目录 `VERSION` 作为单一版本源，HTTP 响应携带 `X-App-Version`。
- 增加 `/healthz`、`/readyz`、`/version`。数据库与 Git 存储未接入时，`/readyz` 返回 503。
- 增加本地 Git 探针，验证 bare 仓库的 clone、非空 commit 和 push。
- 增加最小 React 页面，展示版本并提供中英文 i18n。
- 增加本地进程探针，验证工作目录约束、超时和取消。
- 增加本地 Agent fixture，验证事件契约、空 diff 拒绝和取消后无新副作用。
- 增加 Colima Docker 探针，按镜像 digest 启动 Compose，并核对实际镜像 ID 与 Docker socket 未挂载。
- 增加 Colima Linux VM 内的 rootless 探针，验证 rootless 身份、用户数据目录、BuildKit 构建和容器内无 Docker socket。
  - 增加 Colima Linux VM 内的 OpenSSH forced command 探针，验证服务端身份传递、命令白名单、并发和取消。这是单用户临时 sshd 证据，不传输真实 Git pack。
- 确认业务数据库只支持 PostgreSQL，不做其他数据库适配。
- 增加 `musehub-migrate` 和成对的 `0001_common`、`0002_identity` migration。隔离 PostgreSQL 17 已验证 up、down、up，以及身份表时间戳、更新触发器和外键索引。Web 启动不执行 migration；其余业务表尚未落地。

## [0.0.0-docs1] — 2026-10-30

### Added

- 初始化独立 Git 仓库，托管地址为 `github.com/niuwoai/musehub`，建立 `main` 与 `develop` 分支。
- 提交产品需求、技术架构、数据库、MCP、开发计划、竞品调研和文档检查报告。
- 添加 Rust/React 构建产物、本地环境配置及私钥文件的基础 Git 忽略规则。
- `v0.0.0-docs1` 仅标记文档快照，不代表已实现或发布产品。

## 设计基线 0.1 — 2026-10-30

### Added

- 确定独立项目名称 MuseHub，整理首版需求、技术架构、数据模型、MCP 契约、开发计划与竞品实践调研。
- 明确目标 `0.1.0-rc1` 是规划版本；截至该日期没有实现或发布。
