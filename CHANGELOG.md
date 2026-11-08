# Changelog

## [Unreleased] — 0.1.0-rc2

开发中版本，不是产品发布。

### Added

- 新增 `musehub-mcp-probe`：基于 rmcp `3.8.0` 的 M0 stdio + Streamable HTTP 互操作探针；独立 stdio/HTTP 客户端矩阵测试与 `docs/m0-rmcp-matrix.*` 证据。

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
