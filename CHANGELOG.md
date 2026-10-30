# Changelog

## [Unreleased] — 0.1.0-rc1

开发中版本，不是产品发布。

### Added

- 建立 Rust workspace：`musehub-core`、`musehub-server`、`musehub-git-probe`。
- 以根目录 `VERSION` 作为单一版本源，HTTP 响应携带 `X-App-Version`。
- 增加 `/healthz`、`/readyz`、`/version`。数据库与 Git 存储未接入时，`/readyz` 返回 503。
- 增加本地 Git 探针，验证 bare 仓库的 clone、非空 commit 和 push。
- 增加最小 React 页面，展示版本并提供中英文 i18n。

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
