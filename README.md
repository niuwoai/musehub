# MuseHub

> AI 时代的内网研发与交付平台。文档基线：0.1，2026-10-30。目标首版：`0.1.0-rc1`（尚未实现、尚未发布）。

GitHub 仓库：[niuwoai/musehub](https://github.com/niuwoai/musehub)。初始文档快照标签：`v0.0.0-docs1`，仅用于标识设计资料，不代表可运行产品发布。

MuseHub 是一个计划采用 Rust 服务端、独立 Runner 和 React 控制台的私有部署项目，连接 Git 托管、Issue、Agent 开发、PR、流水线、Release 和部署。它是独立项目，模型网关只是可配置的外部依赖。

当前交付物是产品与技术设计，不包含可运行服务、已执行 migration、已验证的性能结果或部署成果。下述范围是建议冻结的实现基线；商业定位、许可证和正式版本日期仍待决定。

## 文档导航

| 文档 | 回答的问题 |
|---|---|
| [产品概览](docs/product-overview.md) | 产品是什么，首版交付什么 |
| [产品需求与验收](docs/01-product-requirements.md) | 用户流程、功能边界、权限、非功能指标 |
| [技术架构](docs/02-architecture.md) | Rust 模块、Git 接入、Runner、状态机、一致性与部署 |
| [数据库设计](docs/03-database.md) | 表、字段、约束、索引、事务与数据生命周期 |
| [MCP 接口契约](docs/04-mcp-api.md) | 工具清单、输入输出、权限、幂等、错误与长任务 |
| [开发与验证计划](docs/05-development-plan.md) | 里程碑、工作包、依赖、估算与退出条件 |
| [同类产品与实践调研](docs/06-landscape-and-practice.md) | 当前有哪些类似产品，值得借鉴什么，怎样试用 |
| [决策记录](docs/07-decisions.md) | 已采用的默认方案、取舍和待定问题 |

建议先读产品概览和调研，再审查首版范围，最后进入架构、数据与接口设计。文档编号中的需求编号用于关联开发任务和验收证据。

## 首版验收主线

创建带验收标准的 Issue → 保存实现计划 → 授权 Agent 执行 → 独立分支与提交 → PR → 人工审查与 CI → 合并 → 对合并后 commit 构建 → Release → 测试环境部署 → 健康检查 → 结果回写。

交付关系必须可反向查询：部署 → 制品摘要 → 构建与 commit → PR → Issue 版本 → AgentRun。

## 设计基线

- 服务端、调度器、Runner、MCP 适配层使用 Rust；React + TypeScript + i18n 负责 Web。
- 第一版使用标准 Git 子进程实现协议与写操作，不从零实现 pack/协议引擎。
- PostgreSQL 保存业务与调度状态；bare Git 仓库保存代码；本地制品存储作为首版默认，OCI 镜像放外部私有 Registry。
- 模块化单体控制面 + 独立 Linux Runner；构建与 Agent 工作空间隔离；部署 Runner 独立授权。
- 第一版部署目标为单台 Linux 上的 Docker Compose 应用；先支持测试环境与人工批准的受保护环境。
- 一个通用受控进程适配协议，一个完成实测的 Agent 集成；不承诺所有 CLI、模型通用。
- UI、REST、MCP 共用领域服务和授权策略；MCP 不能绕过分支保护与发布策略。

## 后续仓库结构（规划）

```text
musehub/
  Cargo.toml                 # 实现阶段建立 Rust workspace
  crates/{domain,application,store,git,server,runner,agent,mcp,cli}/
  web/                       # React / Vite / TypeScript / yarn / i18n
  migrations/                # up/down 成对，尚未生成
  deploy/                    # Docker Compose 安装与升级配置
  tests/                     # 协议、集成、故障恢复、端到端
  docs/
```

初始仓库包含 `main` 和 `develop`；后续开发分支从 `develop` 派生。代码实现阶段统一版本源并生成 Web 展示版本、二进制版本和 `X-App-Version`。当前仓库仅包含设计文档和基础忽略规则，尚未初始化 Rust workspace 或实现服务。
