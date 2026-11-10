# MuseHub

> AI 时代的内网研发与交付平台。文档基线：0.1，2026-10-30。开发版本：`0.1.0-rc4`（开发中，尚未发布）。

GitHub 仓库：[niuwoai/musehub](https://github.com/niuwoai/musehub)。初始文档快照标签：`v0.0.0-docs1`，仅用于标识设计资料，不代表可运行产品发布。

MuseHub 是可私有部署的研发与交付控制面：Rust API、React 控制台、PostgreSQL 状态、bare Git 与独立 **Runner** worker。模型网关为外接依赖。**当前开发版本见 `VERSION`（`0.1.0-rc4`）**：账号/组织/PAT、Git HTTP、Issue·计划·Agent（reference / codex-sim）、PR·门禁、流水线、Release、Compose 部署、审计与产品 MCP 路径已在 Linux CI 与 acceptance 脚本中验证，可通过 **单机 Docker Compose** 安装使用。

**诚实边界（需运营方自行补齐，本仓库不代为验收）：** 公网 **真实 TLS** 证书与 DNS、控制面与 Runner **另一台物理机** 的长期运行、**Live Codex** 在线凭据、OAuth/SAML、多区域 HA。商业许可证与正式发布日期待定。细粒度进度见 [docs/PROGRESS.md](docs/PROGRESS.md)。

## 自托管安装（单机 Linux · Compose）

面向首名付费租户的推荐路径（真实产品栈，非演示 mock profile）：

| 步骤 | 命令 / 文档 |
|------|----------------|
| 1. 克隆与依赖 | Docker Engine + Compose v2、`jq`、`curl` |
| 2. 生产控制面 | `./scripts/install-production-compose.sh` → 生成 `deploy/.env`（**保存输出的 admin 密码**） |
| 3. Runner PAT + worker | `./scripts/configure-production-runner-pat.sh --start`（写入 `MUSEHUB_RUNNER_PAT` 并启动 `musehub-runner`） |
| 4. 公网/内网 Git 地址 | 在 `deploy/.env` 设置 `MUSEHUB_PUBLIC_HTTP_ORIGIN`（如 `https://musehub.your.corp`），与网关入口一致，UI 中 `clone_url` 才可用 |
| 5. 打开 UI | 浏览器访问 `http://<主机>:${MUSEHUB_GATEWAY_HTTP_PORT:-8080}`，使用 `admin` 登录 |
| 6. 升级 / 迁移 / 备份 | `./scripts/upgrade-production-compose.sh`；日常迁移由容器 entrypoint 自动 `up`；`scripts/backup-musehub-state.sh` |

实验室 **demo profile**（API 进程内执行器、compose-mock）：`deploy/RUNBOOK.md` Quick start 或 `./scripts/verify-clean-install-runbook.sh`。

**尚未包含 systemd 单元**；Compose `restart: unless-stopped` 承担进程守护。外置 OCI Registry 地址在 `deploy/.env` 设置 `MUSEHUB_OCI_REGISTRY`（生产 profile 默认要求 pipeline OCI）。

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
  VERSION                    # 单一版本源（当前见 VERSION 文件）
  Cargo.toml                 # workspace：core、server、探针、migrate
  crates/musehub-core/       # 版本、配置与稳定错误码
  crates/musehub-server/     # 健康检查 + 产品 API（登录/仓库/Git HTTP/Issue/Agent）
  crates/musehub-app/        # 产品领域逻辑（REST）
  crates/musehub-agent-adapter/ # Agent 适配层 + reference 二进制
  crates/musehub-migrate/    # 显式 PostgreSQL migration；Web 启动不调用
  crates/musehub-git-probe/  # 本地 bare 仓库 clone/commit/push 探针
  crates/musehub-runner-probe/ # 本地进程隔离、超时与取消；不是 rootless 证据
  crates/musehub-container-probe/ # Colima Docker digest 与 socket 检查；不是 rootless 证据
  crates/musehub-rootless-probe/ # Colima Linux VM 内的单用户 rootless 证据
  crates/musehub-ssh-probe/  # Colima Linux VM 内的单用户 OpenSSH forced command 证据
  crates/musehub-agent-probe/  # 本地 fixture 契约；不是生产 Agent 集成
  crates/musehub-mcp-probe/  # M0 rmcp stdio + Streamable HTTP 互操作探针；不是生产 MCP 服务
  migrations/                # 0001–0027（身份、仓库、Issue、流水线、MCP 契约等）
  docs/PROGRESS.md           # 真实 P0 进度清单
  web/                       # React / Vite / TypeScript / yarn / i18n
  docs/
```

产品栈安装见 `deploy/`（`docker-compose.yml`、`RUNBOOK.md`、TLS `Caddyfile` 示例）；CI `product-stack` 从干净构建验网关静态站 + API 主线。独立 `runner`、审计、回滚等见 `docs/PROGRESS.md`。

初始仓库包含 `main` 和 `develop`；后续开发分支从 `develop` 派生。根目录 `VERSION` 是版本源：Cargo 包版本、Web 展示版本和响应头 `X-App-Version` 都与它一致。

## 本地验证

复制 `.env.example` 了解配置。产品 API 需要 **`DATABASE_URL` + `MUSEHUB_GIT_ROOT` + `MUSEHUB_SESSION_SECRET`**；一次性引导可设 `MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD`（创建 `demo` 组织与 `admin` 用户）。服务启动**不**自动 migration。

```sh
# 数据库（示例）
export DATABASE_URL=postgres://localhost/musehub
export MUSEHUB_GIT_ROOT=/tmp/musehub-git
export MUSEHUB_SESSION_SECRET=$(openssl rand -hex 32)
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=dev-change-me

cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"
cargo run -p musehub-server

yarn --cwd web install --frozen-lockfile
yarn --cwd web dev   # 代理 /api 与 /git

# 测试
./scripts/cargo-test-portable.sh
./scripts/setup-mcp-matrix-python-deps.sh   # MCP 矩阵所需
cargo test -p musehub-app --test product_slice   # 需 Postgres（CI 提供）

# 状态备份/恢复（运维首版；需 pg_dump/pg_restore）
./scripts/backup-musehub-state.sh /tmp/musehub-backup
./scripts/restore-musehub-state.sh /tmp/musehub-backup   # 破坏性恢复
./scripts/verify-backup-restore.sh                        # PG + bare main OID + artifact SHA
./scripts/verify-compose-runner-e2e.sh                    # 需 Docker：真实 pull/up/安全 inspect（CI 门禁）
```

## 验收主线（已跑 vs 未跑）

| 步骤 | 状态 | 证据 |
|------|------|------|
| 参考 Agent 端到端 | `[x]` | `product_slice`、`run-web-e2e.sh`；远程 worker：`MUSEHUB_AGENT_EXECUTOR=remote` + `verify-remote-agent-e2e.sh` |
| PR / 流水线 / mock 部署 / trace | `[x]` | `pipeline_delivery_e2e` |
| 备份恢复 | `[x]` | `verify-backup-restore.sh`（含 Git OID） |
| **Compose runner 真实部署** | `[x]` CI only | `compose-runner-e2e`：pull 解析 `@sha256`、无 digest 即失败；**非**用户 Release OCI |
| **Pipeline OCI → Release → runner 产品路径** | `[x]` CI | `pipeline-oci-product`：本地 registry build/push、双 commit 不同 digest、`pipeline_oci_product_e2e` + `MUSEHUB_REQUIRE_PIPELINE_OCI=1` 浏览器 E2E |
| Compose **经产品 API** mock 回退 | `[x]` 显式拒绝 | `MUSEHUB_REQUIRE_PIPELINE_OCI=1` 时禁止 compose-mock / mock health；无 OCI 制品不得发布 |
| **生产 compose 远程 worker 全链** | `[x]` 本地 | `verify-product-production-stack-e2e.sh`；`install-production-compose.sh` |
| OCI / 多环境 / 业务 MCP | `[~]` | **51** 工具 + **22** 写幂等 + **5** §13 读；`writes_thin`（merge/respond/pipeline 幂等）+ `resources_v1_*`（含跨 org）；**无** OAuth 远程 |

`/readyz` 在 DB 与 `MUSEHUB_GIT_ROOT` 均就绪时为 200，并暴露 `compose_adapter_mode`（产品镜像默认 **mock**）与 `pipeline_executor`（默认 **in_process**；远程 worker 为 `MUSEHUB_PIPELINE_EXECUTOR=remote`）。

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
- **Linux CI / 本地 Postgres**：`verify-product-acceptance.sh` 与 `musehub-migrate` 测试对 **全量** migration（0001–0027）执行 up → 产品切片 → down → up；`0001_common.down.sql` 先按函数名显式拆除 `musehub_set_updated_at` 触发器再 `DROP FUNCTION`（不用 `CASCADE`）。可选 `MUSEHUB_MIGRATE_TERMINATE_IDLE_PEERS=1` 仅结束本库 **空闲** 客户端连接，缓解 acceptance 并发池化导致的 down 偶发失败，不默认在生产开启。
- **macOS 开发者机**（Homebrew PostgreSQL@17）：`musehub-migrate` 隔离集群测试仍只捆绑 `0001`+`0002` 子集（见 `up_down.rs`），与全量 acceptance 路径不同。
- `musehub-mcp-probe` 在 Linux 本地用 **两种独立 SDK 客户端**（`rust-rmcp` 3.8.0 + 官方 Python `mcp` 2.3.0）各覆盖 stdio 与 Streamable HTTP；见 `docs/m0-rmcp-matrix.md`。这是 **M0 互操作探针**（`m0_mcp_interop_matrix_passed` 由 `cargo test -p musehub-mcp-probe --test matrix` 验证），不是生产 `/mcp`；写工具幂等 **未验证**；OAuth 不在范围。

尚未完成，不能把 M0 标为通过：

- 真实 Agent 适配器选型，以及真实任务的提交、取消和预算证据。当前 Agent 探针只是 fixture。
- 生产级多租户隔离。当前 rootless 与 OpenSSH 证据只覆盖 Colima Linux VM 内的单个普通用户和临时 sshd，不代表不可信贡献者隔离，也不是选定的 Git 服务实现。
- 生产 MCP 服务（完整工具目录、领域授权、写工具幂等、OAuth）与正式兼容矩阵发布。
- 同类产品最小任务试用和差异结论。

**环境相关测试（本机证据，Linux CI 可能失败且未用 skip 掩盖）：** Colima/Homebrew 绑定的 `musehub-container-probe`、`musehub-rootless-probe`、`musehub-ssh-probe` 集成测试，以及 Homebrew PostgreSQL@17 上的 `musehub-migrate` 集成测试。若在 macOS 开发者机上跑全量 `cargo test --workspace`，需具备对应路径的二进制。
