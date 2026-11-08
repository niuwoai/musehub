# MuseHub

> AI 时代的内网研发与交付平台。文档基线：0.1，2026-10-30。开发版本：`0.1.0-rc3`（开发中，尚未发布）。

GitHub 仓库：[niuwoai/musehub](https://github.com/niuwoai/musehub)。初始文档快照标签：`v0.0.0-docs1`，仅用于标识设计资料，不代表可运行产品发布。

MuseHub 是一个计划采用 Rust 服务端、独立 Runner 和 React 控制台的私有部署项目，连接 Git 托管、Issue、Agent 开发、PR、流水线、Release 和部署。它是独立项目，模型网关只是可配置的外部依赖。

当前已有设计文档、最小 Rust 服务、最小 Web，以及只覆盖身份表的 PostgreSQL migration。没有账号功能、Git 托管、流水线、Agent、Release 或部署，也没有已验证的性能结果。下述范围仍是首版目标；商业定位、许可证和正式版本日期仍待决定。

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
- 业务数据库只支持 PostgreSQL，保存业务与调度状态；bare Git 仓库保存代码；本地制品存储作为首版默认，OCI 镜像放外部私有 Registry。
- 模块化单体控制面 + 独立 Linux Runner；构建与 Agent 工作空间隔离；部署 Runner 独立授权。
- 第一版部署目标为单台 Linux 上的 Docker Compose 应用；先支持测试环境与人工批准的受保护环境。
- 一个通用受控进程适配协议，一个完成实测的 Agent 集成；不承诺所有 CLI、模型通用。
- UI、REST、MCP 共用领域服务和授权策略；MCP 不能绕过分支保护与发布策略。

## 当前仓库结构

```text
musehub/
  VERSION                    # 单一版本源，当前 0.1.0-rc2
  Cargo.toml                 # workspace：core、server、探针、migrate
  crates/musehub-core/       # 版本、配置与稳定错误码
  crates/musehub-server/     # /healthz、/readyz、/version
  crates/musehub-migrate/    # 显式 PostgreSQL migration；Web 启动不调用
  crates/musehub-git-probe/  # 本地 bare 仓库 clone/commit/push 探针
  crates/musehub-runner-probe/ # 本地进程隔离、超时与取消；不是 rootless 证据
  crates/musehub-container-probe/ # Colima Docker digest 与 socket 检查；不是 rootless 证据
  crates/musehub-rootless-probe/ # Colima Linux VM 内的单用户 rootless 证据
  crates/musehub-ssh-probe/  # Colima Linux VM 内的单用户 OpenSSH forced command 证据
  crates/musehub-agent-probe/  # 本地 fixture 契约；不是生产 Agent 集成
  crates/musehub-mcp-probe/  # M0 rmcp stdio + Streamable HTTP 互操作探针；不是生产 MCP 服务
  migrations/                # 0001 common、0002 identity；其余表尚未落地
  web/                       # React / Vite / TypeScript / yarn / i18n
  docs/
```

规划中的 `application`、`store`、完整 Git 服务、`runner`、`agent`、`mcp`、`cli` 和 `deploy/` 都还没有。`migrations/` 目前只有公共函数和身份表。

初始仓库包含 `main` 和 `develop`；后续开发分支从 `develop` 派生。根目录 `VERSION` 是版本源：Cargo 包版本、Web 展示版本和响应头 `X-App-Version` 都与它一致。

## 本地验证

复制 `.env.example` 了解可选配置。服务读取 `MUSEHUB_BIND`（默认 `127.0.0.1:8080`）和可选 `DATABASE_URL`（仅 PostgreSQL）。配置数据库后，`/readyz` 通过 `SELECT 1` 检查连接；未配置或连接异常时服务仍启动并报告未就绪。服务启动不执行 migration。

```sh
cargo test --workspace
# MCP 互操作矩阵（stdio 子进程客户端 + Streamable HTTP 客户端，无付费依赖）：
cargo test -p musehub-mcp-probe --test matrix
cargo run -p musehub-server
yarn --cwd web install --frozen-lockfile
yarn --cwd web test
yarn --cwd web build
```

`/healthz` 只表示进程存活。Git 存储尚未接入，因此即使 PostgreSQL 检查通过，`/readyz` 仍返回 HTTP 503。

## M0 进度

已验证：

- Rust workspace、单一版本源、配置错误和 Git 探针错误码。
- 结构化日志，以及 `/healthz`、`/readyz`、`/version`。
- 标准 Git 子进程完成本地 bare 仓库的 clone、非空 commit 和 push。
- 最小 Web 从 `VERSION` 读取版本，中英文文案来自 i18n。
- 本地进程探针覆盖工作目录约束、超时终止和取消后不再写文件。
- 本地 fixture 覆盖 NDJSON 事件、空 diff 拒绝，以及取消后没有新文件或提交。
- Colima Docker VM 上按 busybox digest 启动 Compose，实际运行镜像 ID 与请求 digest 的镜像 ID 一致，容器没有挂载 Docker socket。
- Colima Linux VM 内的独立 rootless Docker 报告 `name=rootless`，数据目录位于用户目录，BuildKit 构建成功，容器内看不到 Docker socket。
- Colima Linux VM 内的临时 OpenSSH 验证 forced command：服务端注入身份，客户端不能覆盖；只接受明确的 `git-upload-pack` / `git-receive-pack` 与一个逻辑路径；两个主体并发时各自看到自己的身份；客户端提前断开后没有仓库文件或提交。没有执行真实 Git pack。
- `musehub-migrate` 对隔离的 PostgreSQL 17 执行 `0001_common` 与 `0002_identity` 的 up、down、up。身份表具备 `created_at`、`updated_at` 与更新触发器，外键有前缀索引。Web 启动不迁移；仓库、Issue、流水线等其余表仍未落地。
- `musehub-mcp-probe` 在 Linux 本地用 **两种独立 SDK 客户端**（`rust-rmcp` 3.8.0 + 官方 Python `mcp` 2.3.0）各覆盖 stdio 与 Streamable HTTP；见 `docs/m0-rmcp-matrix.md`。这是 **M0 互操作探针**（`m0_mcp_interop_matrix_passed` 由 `cargo test -p musehub-mcp-probe --test matrix` 验证），不是生产 `/mcp`；写工具幂等 **未验证**；OAuth 不在范围。

尚未完成，不能把 M0 标为通过：

- 真实 Agent 适配器选型，以及真实任务的提交、取消和预算证据。当前 Agent 探针只是 fixture。
- 生产级多租户隔离。当前 rootless 与 OpenSSH 证据只覆盖 Colima Linux VM 内的单个普通用户和临时 sshd，不代表不可信贡献者隔离，也不是选定的 Git 服务实现。
- 生产 MCP 服务（完整工具目录、领域授权、写工具幂等、OAuth）与正式兼容矩阵发布。
- 同类产品最小任务试用和差异结论。

**环境相关测试（本机证据，Linux CI 可能失败且未用 skip 掩盖）：** Colima/Homebrew 绑定的 `musehub-container-probe`、`musehub-rootless-probe`、`musehub-ssh-probe` 集成测试，以及 Homebrew PostgreSQL@17 上的 `musehub-migrate` 集成测试。若在 macOS 开发者机上跑全量 `cargo test --workspace`，需具备对应路径的二进制。
