# MuseHub

> AI 时代的内网研发与交付平台。文档基线：0.1，2026-10-30。开发版本：`0.20.0-rc6`（开发中，尚未发布）。

GitHub 仓库：[niuwoai/musehub](https://github.com/niuwoai/musehub)。初始文档快照标签：`v0.0.0-docs1`，仅用于标识设计资料，不代表可运行产品发布。

MuseHub 是可私有部署的研发与交付控制面：Rust API、React 控制台、PostgreSQL 状态、bare Git 与独立 **Runner** worker。模型网关为外接依赖。**当前开发版本见 `VERSION`（`0.20.0-rc6`）：账号/组织/PAT、Git HTTP 与固定提交的代码浏览（含可分页提交历史和父提交 diff；2026-11-17 在本地开发栈通过 Playwright，覆盖 22 条提交历史、含空格路径 diff 与文本文件浏览）、Issue·计划·Agent（reference / codex-sim）、PR·门禁（当前 head 检查与审批状态、讨论支持行评论位置）、流水线、Release、Compose 部署、审计与产品 MCP 路径已在 Linux CI 与 acceptance 脚本中验证，可通过 **单机 Docker Compose** 安装使用。AgentRun 会将启动时的 Issue 修订版、验收标准和已接受计划快照写入任务输入；REST 与 MCP 共用该规则；`plan_immutable` REST 回归验收验证了快照字段与旧修订拒绝启动，`production_mcp_contract_writes_thin_e2e::mcp_agent_run_start_queues_run_idempotent_replay_and_conflict` 也验证 MCP 快照字段、幂等重放及 Issue 更新后拒绝旧计划（2026-11-19，1/1）；`issue-plan-acceptance` Playwright 已在本地开发栈通过，覆盖计划接受、编辑后的启动禁用和重新接受。真实 Codex Runner 验收脚本需显式启用并指定 token 阈值（当前脚本上限为 128,000）；阈值在 Codex 报告 usage 后检查，不保证单回合不越界；AgentRun 终态保存 Codex 上报的最小 `turn.completed.usage` 对象，并校验其 token 总量与 `usage_tokens` 一致，不保存 prompt 或对话输出；普通 CI 不调用在线模型，真实 Codex 在线调用已返回可核对的 usage；第八次真实调用在 128,000-token 预算下报告 106,172 tokens，但仍以 `agent_changes_uncommitted` 结束；八次真实 Codex 调用均未提交；第九次验收尝试因不受支持的 CLI 参数在调用模型前失败，usage 为 0，已替换为本机 CLI 支持的 `--approve-for-me` 并保持 `workspace-write` 沙箱；第十至十二次均以 `agent_adapter_failed`、usage 0 结束；第十一次 Codex CLI 退出状态为 2、stderr 分类为 `unclassified`；第十二次退出状态为 2、固定诊断分类为 `option_conflict`。第十二次未产生可确认的 provider usage 或提交；后续确认旧参数同时传入 `--approve-for-me` 与显式 `--sandbox`，两者互斥。本机 CLI 版本为 0.169.0，完整参数组合可通过 `--help` 解析，登录状态为 authenticated；provider 调用状态仍无法从 usage 确认。现有诊断同时识别 stderr 与 JSONL 错误事件，只记录固定类别、不保存原始输出；此前怀疑的 CLI 参数冲突已确认：`--approve-for-me` 已启用 `workspace-write`，不能再同时传入显式 `--sandbox`；适配器现已移除冲突参数。修复后的真实调用完成一个 Codex turn，但 usage 为 input 178,454、output 588、total 179,042 tokens，超过 128,000-token AgentRun 预算并失败，未产生 commit；随后在适配器忽略 `$CODEX_HOME/config.toml` 的情况下再次运行，usage 为 input 193,317、output 672、total 193,989 tokens，仍超预算且没有 commit。Codex 官方说明 `--approve-for-me` 会启动独立自动审查 reviewer，产生额外计费 token；改为 `approval_policy=never` 同时显式保留 `sandbox_mode=workspace-write` 后，usage 降至 115,272 tokens，但 AgentRun 仍以 `agent_changes_uncommitted` 结束。验收指令已收窄为立即提交目标文件；适配器现通过 Codex `--add-dir` 仅授权临时工作区 Git metadata 写入；2026-11-17 最新在线运行在 128,000-token 预算内成功创建并推送 commit；proof 文件精确匹配，Codex usage 与数据库记录一致，bare 分支指向该 commit，Live Codex 验收通过。

AgentRun 详情会展示冻结 Issue 修订版、已接受计划版本、基准提交、Token 预算、最近执行阶段及可操作的失败说明。Issue 交付时间线串联计划、AgentRun、PR、流水线、Release 和部署；Release 草稿支持专属说明，并将选定的已验证流水线工件写入不可变发布清单。

Workflow job 可声明通用产物与 JUnit 报告；控制面校验并持久化摘要，仓库成员可在交付页查看测试计数和下载授权产物。声明的依赖缓存按组织、仓库、可信级别、镜像摘要和 lockfile hash 隔离，并有 TTL 与总容量上限；缓存不进入 job 容器挂载，也不进入备份/镜像。2026-11-19 在专用 SSH/Runner 隔离栈完成真实 push 触发验收：两个 artifact 经认证下载并通过 SHA-256/内容核验，依赖 job 命中缓存且内容断言通过；同一轮 opt-in T1 产品 Runner 验收使用 SHA-256 固定的 Node 镜像，错误基线 3 项测试失败，修复 PR head 的 Runner 测试 3 项通过、`product-ci` 成功；Runner 到专用 DinD Docker API 的 mTLS 连通检查通过。新建隔离数据库的 migration 0001–0046 完整 `up → down → up`，两次 schema check 均通过。该结果只证明 MuseHub 单产品 Runner 流程，不代表新的三方同基线对照通过；其余 README 规划状态见 [docs/PROGRESS.md](docs/PROGRESS.md)。

**诚实边界（需运营方自行补齐并实测）：** 网关已支持手动证书和 ACME；`scripts/verify-public-tls.sh` 可从外部检查 DNS 解析、HTTP→HTTPS、受信任证书、readyz 和版本一致性，但真实公网域名仍需目标环境验收。另有独立主机 Runner 的 opt-in Compose 模板，HTTP/SSH push、合并和 AgentRun 发布已加入 PostgreSQL 跨进程仓库写锁；双连接池争用验收 `repository_lock_contention_e2e` 已在一次性 PostgreSQL 17 容器通过；独立 Linux Runner 已在跨容器、host-network 与共享 DB/卷场景验收；物理双机长期运行是额外部署拓扑，不是 V1 Runner 验收门槛。SSH 安全矩阵已有跨仓库无 grant 拒绝用例，但不证明不可信代码执行隔离。现已提供组织范围的 SCIM Users/Groups 同步子集，含最多 100 项/1 MiB 的有界 Bulk（支持已创建资源的顺序 bulkId 引用，不支持前向引用或密码同步；Group 成员关系仅作为目录数据，不授予 MuseHub 权限；handler 错误按 RFC 7644 格式返回），以及实验性的 SP 发起 SAML SSO：要求 IdP 元数据由固定 PEM 证书验签，登录身份必须由管理员预先绑定；2026-11-17 已在 Keycloak 26.4.2 上完成本地浏览器登录，POST ACS 返回 303、MuseHub 会话建立且 `/api/v1/me` 返回 200。由于 Keycloak 原始 realm metadata 未签名，本次使用补根 ID、由本地临时测试证书签名并声明不要求 AuthnRequest 签名的合成 metadata fixture；真实企业 IdP 互操作按当前范围延期；V1 外部登录最低要求为 GitHub OAuth App（登录代码已实现；真实 App 凭据与线上 callback 尚待验收），所用 SAML crate 尚未到 1.0 且未声明外部安全审计。单 Provider OIDC 登录支持用户在账户安全页将经验证的 `(issuer, subject)` 绑定到自身 MuseHub 账号；隔离 Keycloak 26.4.2 上已完成 OIDC 授权码/PKCE 登录、未绑定身份拒绝、会话和审计核验，但这不代表真实企业 IdP 验收。共享 Git/制品存储现提供 opt-in Compose overlay 与双主机可见性、原子改名、`flock` 排他及源主机停机后的 survivor-check；另新增 SSH/rsync 异步单向镜像脚本和每分钟 systemd timer，每轮按内容校验并记录最后成功时间，支持静默后的校验/删除收敛；一次性 Alpine 容器已验证本地镜像行为、容器内 SSH/rsync、PostgreSQL 17 主备 WAL 追平屏障、滞后拒绝、备用库提升和屏障迁移回滚（不含独立远端主机、真实远端 RPO、物理 fencing 或区域故障切换；一次性容器中的 systemd daemon 未能保持运行；2026-11-17 在独立 Colima VM 的 systemd 255 中安装 MuseHub mirror unit，local-gated timer 在启动与 guest reboot 后均成功同步 Git/artifacts 并写入成功状态；仅验证单 VM 本地目标，不代表远端复制或生产灾备验收）。该副本不支持双写、不复制 PostgreSQL，真实双机的 RPO、停机收敛与提升仍未验收，共享挂载本身也不提供复制或自动故障转移。新增 CloudNativePG 三实例单区域 HA 集群参考清单（同步复制、failover quorum、跨 zone 反亲和及私有 writer LB），另有外部 PostgreSQL HA endpoint overlay、primary/standby readiness 和 standby 变更请求 503 门禁，并提供 failover 后检查脚本（验证新主库、旧主库不可写及全部 API 实例 readiness/version）。2026-11-17 已在本机临时 k3d 集群验证 Pod 故障切换：删除 primary Pod 后副本自动提升，集群恢复 3/3 Ready，已确认同步提交在新主库仍存在；另停掉承载 primary 的 k3d worker 容器后，主库提升至 `musehub-postgres-2`、集群恢复 2/3 Ready，已确认记录仍保留。两项均为单机 Docker/k3d 模拟，不验证真实物理节点故障、网络分区 fencing、真实云存储、writer/API 流量切换或生产级灾难恢复。另提供 CloudNativePG Barman Cloud 异地备库配置与 demotionToken 受控提升脚本；新旧 region 的集群/对象存储配置仍需按环境部署，旧主 fencing 由云平台或集群管理面完成，生产 RPO、跨区切流和灾备演练尚未验收。已新增基于 rclone 的异地备份快照复制与下载校验，并提供下载、manifest/归档路径校验后落盘的 `materialize-musehub-backup.sh`；该路径不提供实时文件复制或自动切换，且一致快照要求业务写入静默窗口，独立 PostgreSQL 17.7 临时容器已通过本地 backup→restore，核对 DB 行、Git commit OID 和制品 SHA；2026-11-17 使用 rclone 1.76.0 本地 filesystem backend 运行 `materialize-musehub-backup.sh`，4 个文件校验匹配、manifest/归档路径检查通过且源目标目录一致，这不代表远端 bucket 验收。本机临时 S3-compatible SeaweedFS 已通过仓库复制/物化脚本验证安全 fixture 上传、下载字节比对、manifest 与归档路径校验；fixture 使用 README 副本，不是实际 PostgreSQL/Git/制品快照恢复。真实云 bucket 的复制与恢复仍未验收。SSH/rsync 在线文件镜像与真实故障恢复、多区域 HA 生产部署/演练和完整 SCIM 互操作仍未在目标基础设施验收；仓库现提供异步单向文件镜像服务、共享存储幸存节点验收命令与多区域 PostgreSQL DR 配置/受控提升流程。Live Codex 显式验收曾因 macOS 缺少 `musehub-git` 组而在仓库创建时返回 HTTP 500；修复仓库创建路径后，2026-11-17 的真实 AgentRun 已运行七次，provider usage 总量依次为 105,798、76,733、96,612、76,991、57,268、96,800、106,172 tokens；第一和第七次超过 100,000 阈值，其余五次在阈值内但未提交。第六次诊断为 `agent_changes_uncommitted`；第七次以 `agent_budget_exceeded` 结束，未能验证临时 Git author identity fallback。usage evidence 与数据库 `usage_tokens` 一致；此前旧版对累计快照逐轮相加形成的 `141139` 不可信，`140516` 也因缺少原始 JSONL 而无法独立核验。未提交根因仍未证实，2026-11-17 已完成在线成功提交验收：AgentRun `d9528061-40c5-46f3-9e39-197c3c230e40` 推送 commit `9ae13fd16876c84b769b33f3cc68140ea5fd345f`，proof 精确匹配，provider usage 77,807 与数据库一致。预算在 usage 到达后检查，不能阻止单回合先超额。商业许可证与正式发布日期待定。细粒度进度见 [docs/PROGRESS.md](docs/PROGRESS.md)。

## 自托管安装（单机 Linux · Compose）

面向首名付费租户的推荐路径（真实产品栈，非演示 mock profile）：

| 步骤 | 命令 / 文档 |
|------|----------------|
| 1. 克隆与依赖 | Docker Engine + Compose v2、`jq`、`curl` |
| 2. 生产控制面 | `./scripts/install-production-compose.sh` → 生成 `deploy/.env`（**保存输出的 admin 密码**） |
| 3. Runner PAT + worker | `./scripts/configure-production-runner-pat.sh --start`（写入 `MUSEHUB_RUNNER_PAT` 并启动 `musehub-runner`） |
| 4. 公网/内网 Git 地址 | 在 `deploy/.env` 设置 `MUSEHUB_PUBLIC_HTTP_ORIGIN`（HTTP 或 HTTPS，与网关入口一致），UI 中 `clone_url` 才可用 |
| 5. HTTPS 网关（生产可选） | `MUSEHUB_GATEWAY_TLS_MODE=manual\|acme`、域名与证书路径见 `deploy/.env.example` 与 `deploy/RUNBOOK.md`；`./scripts/render-gateway-caddyfile.sh` 后 `./scripts/upgrade-production-compose.sh`；TLS 开启时 **`MUSEHUB_PUBLIC_HTTP_ORIGIN` 应为 `https://…`**；从外部网络运行 `scripts/verify-public-tls.sh` |
| 6. 打开 UI | 默认 HTTP：`http://<主机>:${MUSEHUB_GATEWAY_HTTP_PORT:-8080}`；TLS 已启用时用 `https://<MUSEHUB_GATEWAY_TLS_DOMAIN>`；控制台 **Members / PAT**（owner 才能管理 owner；邀请不可降权；**禁用用户为全局 principal**，非仅当前 org） |
| 7. 升级 / 迁移 / 备份 | `./scripts/upgrade-production-compose.sh`；迁移由容器 entrypoint 自动 `up`；`scripts/backup-musehub-state.sh` + `scripts/replicate-musehub-backup.sh` |
| 8. 主机重启后自启（可选） | `sudo ./scripts/install-musehub-systemd.sh` → `systemctl enable --now musehub-control-plane`；`musehub-runner` 仅在 `MUSEHUB_RUNNER_PAT` 已配置时实际启动 worker |
| 9. GitHub 登录（可选） | 按 [`deploy/RUNBOOK.md`](deploy/RUNBOOK.md) 创建 OAuth App，配置三个 `MUSEHUB_GITHUB_OAUTH_*` 环境变量并重建 server |

实验室 **demo profile**（API 进程内执行器、compose-mock）：`deploy/RUNBOOK.md` Quick start 或 `./scripts/verify-clean-install-runbook.sh`。

异步 Git/制品镜像是独立可选服务：先配置 `/etc/musehub/storage-mirror.env`，再运行 `sudo ./scripts/install-musehub-storage-mirror.sh`；仅在主节点使用 `--enable` 启动 timer。步骤与双机限制见 `deploy/RUNBOOK.md`。容器内 `restart: unless-stopped` **不能**代替宿主机在 Docker 就绪后拉起整栈；首日运维见上。外置 OCI Registry：`deploy/.env` 中 `MUSEHUB_OCI_REGISTRY`。

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
  migrations/                # 0001–0050（含 workflow artifacts、Issue/OIDC、Release 完整性与 AgentRun 进度）
  docs/PROGRESS.md           # 真实 P0 进度清单
  web/                       # React / Vite / TypeScript / yarn / i18n
  docs/
```

产品栈安装见 `deploy/`（`docker-compose.yml`、`RUNBOOK.md`、TLS `Caddyfile` 示例）；CI `product-stack` 从干净构建验网关静态站 + API 主线。租户范围的审计驱动 HTTPS Webhook 已进入首版（组织管理员管理订阅、HMAC 签名、持久化重试；密钥需单独配置 `MUSEHUB_WEBHOOK_ENCRYPTION_KEY`），不含 SMTP/收件箱。独立 `runner`、审计、回滚等见 `docs/PROGRESS.md`。

初始仓库包含 `main` 和 `develop`；后续开发分支从 `develop` 派生。根目录 `VERSION` 是版本源：Cargo 包版本、Web 展示版本和响应头 `X-App-Version` 都与它一致。

## 本地开发（推荐入口）

**目标：** 打开浏览器即可登录控制台，而不是 Vite 静态壳 + `/readyz` 503。  
**边界：** 本脚本只起 **loopback** 上的 API + Vite；执行器默认为 **进程内 + compose-mock**（与生产 Compose + remote runner **不是**同一 profile）。

1. **PostgreSQL**（任选其一，需先可达）  
   - macOS Homebrew：`brew install postgresql@17 && brew services start postgresql@17` 后自建库/用户。  
   - Docker（示例）：`docker run -d --name musehub-pg -e POSTGRES_USER=musehub -e POSTGRES_PASSWORD=musehub -e POSTGRES_DB=musehub_dev -p 5432:5432 postgres:17`（`dev-local-stack.sh init` 在未设置 `DATABASE_URL` 时默认 `postgres://musehub:musehub@127.0.0.1:5432/musehub_dev`；CI 通过环境变量覆盖为 `musehub`。）

2. **一键流程**（状态写在 `.local/dev-stack/`，**不**覆盖已有 `env`，**不**清库）：

```sh
./scripts/dev-local-stack.sh init    # 生成 .local/dev-stack/env (600) + migrate
./scripts/dev-local-stack.sh start   # 等 /readyz.ready==true 再起 Vite
./scripts/dev-local-stack.sh status  # 无密钥输出
./scripts/dev-local-stack.sh stop    # 只停本脚本记录的 pid
```

- 首次 bootstrap 密码文件：`.local/dev-stack/secrets/bootstrap-admin.password`（`chmod 600`），登录 **`admin` / 该文件内容**，组织 **`demo`**。  
- UI：<http://127.0.0.1:5173/login>（API <http://127.0.0.1:8080/readyz>）。  
- macOS 生产安装脚本中的 `sed -i` 已改为 `scripts/lib/sed-portable.sh`（GNU/BSD 兼容）。

CI 冒烟：`./scripts/verify-dev-local-stack-negative.sh` + `./scripts/verify-dev-local-stack.sh`（需 `DATABASE_URL`、`pg_isready`、`jq`、`curl`、`openssl`；Linux CI 已验，macOS 未在 Agent 环境实测）。

真实 Codex Runner 验收是**可选且可能产生模型费用**的操作，不在 CI 中运行。需先准备专用的 PostgreSQL 验收数据库并设置 `MUSEHUB_TEST_DATABASE_URL`（脚本会执行迁移并留下验收记录，不要指向生产数据库），配置 Codex CLI 登录，然后显式设置 token 阈值。该阈值在 Codex 报告每回合累计用量后检查，**不是单回合硬上限**，实际消耗可能先超过设置值：

```sh
env MUSEHUB_LIVE_CODEX=1 MUSEHUB_TEST_DATABASE_URL=postgres://musehub:musehub@127.0.0.1:5432/musehub_live_acceptance MUSEHUB_AGENT_CODEX_BIN="$(command -v codex)" MUSEHUB_LIVE_CODEX_BUDGET_TOKENS=128000 ./scripts/verify-live-codex-agent-e2e.sh
```

脚本会创建临时仓库和 AgentRun，要求真实 Codex 在指定分支提交验收文件，并校验 run、commit 与 token 使用量。未执行该脚本不代表 Live Codex 已验收。

## 本地验证（手工 / 高级）

复制 `.env.example` 了解配置。产品 API 需要 **`DATABASE_URL` + `MUSEHUB_GIT_ROOT` + `MUSEHUB_SESSION_SECRET`**；一次性引导可设 `MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD`（创建 `demo` 组织与 `admin` 用户）。服务启动**不**自动 migration。

### API 性能验收

`docs/01-product-requirements.md` §7 的 API 性能目标可用 `scripts/benchmark-api.py` 实测。先在专用预热环境准备至少 50 名活跃组织成员和 100 个仓库，设置 `MUSEHUB_PERF_PASSWORD`，再运行：

```sh
python3 scripts/benchmark-api.py --origin http://127.0.0.1:8080 --org demo --rps 100 --duration-seconds 60 --min-members 50 --min-repositories 100 --control-plane-vcpu 4 --control-plane-memory-gib 8 --ssd-declared yes --concurrent-jobs-declared 10 --output-prefix .local/perf/api-$(date -u +%Y%m%dT%H%M%SZ)
```

脚本使用登录会话，API 负载只读取组织成员、仓库列表和 Issue 列表；它会预检数据规模、预热 API，再报告请求数、错误率、吞吐和 p50/p95/p99，并输出 JSON 与 Markdown。机器规格由运行者声明，不会被脚本验证。调度探针另行真实启动 Runner job，用于测量并发度和调度延迟。没有真实运行报告前，不得将性能目标标记为通过。

如需测量“有空闲 Runner 时调度 p95 < 5 秒”，需在专用隔离仓库准备安全的 `manual` workflow（镜像固定 digest、命令固定且可审查），并确认该提交的 workflow 内容允许真实执行。每次探针会创建持久 PipelineRun 并运行真实 job；仅在隔离环境显式 opt-in 后执行：

```sh
MUSEHUB_PERF_ALLOW_JOB_EXECUTION=1 python3 scripts/benchmark-api.py --origin http://127.0.0.1:8080 --org demo --scheduler-repo perf-runner-isolated --scheduler-source-oid 0123456789abcdef0123456789abcdef01234567 --scheduler-samples 10 --output-prefix .local/perf/scheduler-$(date -u +%Y%m%dT%H%M%SZ)
```

将仓库名和 OID 替换为专用仓库及已审查提交的真实值。探针会快速连续创建 10 个持久 PipelineRun，然后并行轮询；workflow 必须使用固定 digest 镜像、安全且固定的命令，并保持运行足够长，才能观察任务实际重叠。报告同时给出峰值观测并发数和每个 run 首次观察到 `running` 的时间；后者受轮询间隔与 API 响应影响，是调度延迟上界估计，并非 Runner 内部精确启动时刻。只有观察到 10 个 job 同时处于 `running`、全部任务成功且调度 p95 < 5 秒，调度探针才通过。队列或执行超时会触发取消请求并使测量失败；取消是异步操作，超时任务可能仍在收敛。使用新输出前缀，脚本拒绝覆盖已有报告。

```sh
# 数据库（示例）
export DATABASE_URL=postgres://localhost/musehub
export MUSEHUB_GIT_ROOT=/tmp/musehub-git
export MUSEHUB_SESSION_SECRET=$(openssl rand -hex 32)
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=dev-change-me

cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"
cargo run -p musehub-server

yarn --cwd web install --frozen-lockfile
yarn --cwd web dev   # 代理 /api 与 /git → http://127.0.0.1:5173/login

# Web 控制台（Phase 1 路由）
# - 登录入口 `/login`；登录后默认 `/org/demo/repos`；仓库创建后进入该仓库 `/issues`（与点击仓库列表一致）。
# - 仓库子页：`/org/demo/repos/:slug/issues|pulls|code|delivery|settings`；Issue/PR **深链** `.../issues/:n`、`.../pulls/:n`（裸 `/org/demo/repos/:slug` 重定向到 issues）。
# - 账户 `/account/security` 含 **SSH 公钥**管理；Git **HTTPS + PAT** 与 **SSH**（`musehub-git-ssh serve`，见 `deploy/RUNBOOK.md`）均可用于 clone/push。
# - 组织与账户：`/organizations` 可查看当前用户所属组织、创建组织并切换工作区；新组织创建时将创建者设为 owner。组织资源仍由后端 membership 授权。账户：`/account/security|tokens`。
# - 生产 Compose 网关（`:8080`）静态 UI 使用 `deploy/Caddyfile*` 的 `try_files {path} /index.html` SPA 回退（与 Vite dev 深链行为一致）。

# 浏览器 E2E：专用 Postgres、显式 reset 授权与 bootstrap admin 密码；脚本自启 MuseHub 和 Vite
# 先设置 MUSEHUB_TEST_DATABASE_URL、MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1、MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
# 若默认端口被占用，可设置 MUSEHUB_E2E_SERVER_PORT 和 MUSEHUB_E2E_WEB_PORT
# 本机已有 web/node_modules 和 Google Chrome 时，可加 MUSEHUB_E2E_SKIP_INSTALL=1 MUSEHUB_E2E_BROWSER_CHANNEL=chrome
# PLAYWRIGHT_ARGS=e2e/repo-code-console.spec.ts MUSEHUB_E2E_ALLOW_PLAYWRIGHT_ARGS=1 ./scripts/run-web-e2e-isolated.sh
# OIDC 自助绑定（启动固定版本 Keycloak 临时容器；只使用上述专用验收数据库）
# MUSEHUB_TEST_DATABASE_URL=postgres://.../musehub_oidc_link_acceptance MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1 ./scripts/verify-oidc-identity-link-e2e.sh

# 测试
./scripts/cargo-test-portable.sh
./scripts/setup-mcp-matrix-python-deps.sh   # MCP 矩阵所需
cargo test -p musehub-app --test product_slice   # 需 Postgres（CI 提供）

# 状态备份/恢复（运维首版；需 pg_dump/pg_restore）
./scripts/backup-musehub-state.sh /tmp/musehub-backup
MUSEHUB_RESTORE_ALLOW_DESTRUCTIVE=1 ./scripts/restore-musehub-state.sh /tmp/musehub-backup
# 隔离验收只接受专用 musehub_backup_restore 数据库，并自行创建/清理私有临时目录
MUSEHUB_TEST_DATABASE_URL=postgres://musehub:musehub@127.0.0.1:5432/musehub_backup_restore MUSEHUB_BACKUP_RESTORE_ALLOW_RESET=1 ./scripts/verify-backup-restore.sh  # DB 行、权限、Release、制品、审计及全部 Git refs
./scripts/verify-compose-runner-e2e.sh                    # 需 Docker：真实 pull/up/安全 inspect（CI 门禁）
```

## 验收主线（已跑 vs 未跑）

| 步骤 | 状态 | 证据 |
|------|------|------|
| 参考 Agent 端到端 | `[x]` | `product_slice`、`run-web-e2e.sh`；远程 worker：`MUSEHUB_AGENT_EXECUTOR=remote` + `verify-remote-agent-e2e.sh` |
| PR / 流水线 / mock 部署 / trace | `[x]` | `pipeline_delivery_e2e` |
| 受保护环境审批 + 部署列表/历史（控制台） | `[x]` | `deployment_approval_console`、`deployment-list-discovery`、`deployment-history-console`；列表 API `GET .../deployments`（limit/offset/state） |
| 部署失败诊断与回滚（控制台） | `[x]` | `deployment-rollback-console`；REST `POST .../deployments/rollback` + 客户端 `attempt_id` 防重复点击；Playwright 为 **路由 fixture**，不替代 `deployment_rollback_runner_e2e` / 受保护环境 live 审批 |
| 流水线失败诊断与 build job 重试（控制台） | `[x]` | `pipeline_job_retry_http_e2e`、`pipeline-job-retry-console`（**fixture**）；`POST .../pipeline-jobs/{id}/retry` + `attempt_id`；HTTP 验收覆盖普通 job requeue 和 workflow retry（成功节点保留、失败/跳过 DAG 节点创建新 attempt），**非** live worker 执行完成 |
| 流水线运行取消（控制台与 Runner） | `[x]` 控制台与 Linux Runner | `pipeline_cancel_http_e2e`、`pipeline-cancel-console`（**fixture**）；`remote_pipeline_runner_e2e` 实际通过：慢速 Adapter 已启动后发起取消，PID 退出、run/job 终态为 `canceled`、制品被隔离且没有 `bundle.tar`；`cancel_requested` 为观测中状态，**canceled** 才表示终态 |
| Workflow PR/push/manual/tag 触发与 SSH push outbox | `[x]` SSH → Runner 已真实验收 | PR、HTTP ref、manual 触发及 SSH receive-pack heads/tags outbox；Runner 将非 release run 交给 job adapter，adapter 接受 push/tag/manual-only workflow；孤儿 pipeline 恢复时未完成 attempt 标记 `lost`、workflow job 终结为失败或取消；0041–0045 migrations 已于 2026-11-19 在一次性 PostgreSQL 17 数据库通过完整 up/down/up 验收；同日 `verify-ssh-runner-workflow-e2e.sh` 在专用 Colima Docker context 和独立 Compose 项目中完成真实 Git SSH push（OID `5220853453accd782f35d41dbd69a276ab8baee5`）、outbox 派发（`99be06a7-1a39-49f5-a0d6-5261d6c3aeb6`，`completed`）及 DinD Runner 执行；workflow run `ffc00dcc-8439-484b-9623-753aff4838de` 的 run/job/attempt 均为 `succeeded`，依赖 job cache hit 与 payload 断言通过，两个 artifact 经认证下载且 SHA-256 与预期内容一致；脚本仅接受专用 Docker context（通过 `DOCKER_CONTEXT` 选择），且工作区文件系统至少有 10 GiB 可用空间；0042 为各 workflow job/attempt 持久化独立日志；失败 workflow job 可通过 retry API 恢复失败/跳过节点，并保留已成功节点及历史 attempt 日志；控制面每 60 秒扫描已存在 refs 并幂等补入 outbox，以修复 receive-pack 成功后进程崩溃造成的漏事件，首次启用时未有历史 outbox 记录的现存 refs 会各触发一次 |
| PR CI 失败 → Agent 修复（§2 S3） | `[x]` | `GET .../ci-failure`（含失败 job **log_api_path** / **log_excerpt** 来自 `pipeline_job_logs`）+ `GET .../pipeline-jobs/{id}/logs` + `agent-fix`；`pr_ci_failure_log_http_e2e`、`pr_agent_fix_http_e2e`；每 PR 最大轮次默认 **3**；**无** check 独立日志、**非** 自动循环；CI 绿 ≠ §8 生产验收 |
| Issue REST / MCP 产品路径 | `[x]` 本地隔离 E2E | 2026-11-19：`issue_rest_console_http_e2e` 1/1、`issues_create_http_e2e` 1/1、`production_mcp_write_expansion_e2e` 3/3；覆盖 criteria revision、状态/标签/负责人过滤和分页、Issue create/update/list/get、幂等及权限。使用独立 PostgreSQL 17 临时容器；详见 [docs/PROGRESS.md](docs/PROGRESS.md) 与 [docs/ACCEPTANCE-EVIDENCE.md](docs/ACCEPTANCE-EVIDENCE.md) |
| 备份恢复 | `[x]` 本地隔离恢复与复制→物化→恢复联验；`[~]` CI/异地副本 | 2026-11-19 `verify-backup-restore.sh` 在专用 `musehub_backup_restore` 数据库上完成 migrations 0001–0048、schema check、备份生成、rclone 本地 filesystem 复制及 4 文件字节校验（0 differences）、下载物化、恢复，并核对 9 类数据库实体、4 个 Git refs（含 HEAD）、28 个 Git 文件及 artifact SHA-256。数据库仅位于本轮专用 PostgreSQL 容器；GitHub Actions 本轮未运行，filesystem/SeaweedFS fixture 不代表真实云 bucket，生产灾备仍未验收 |
| **Compose runner 真实部署** | `[x]` CI only | `compose-runner-e2e`：pull 解析 `@sha256`、无 digest 即失败；**非**用户 Release OCI |
| **Pipeline OCI → Release → runner 产品路径** | `[x]` CI | `pipeline-oci-product`：本地 registry build/push、双 commit 不同 digest、`pipeline_oci_product_e2e` + `MUSEHUB_REQUIRE_PIPELINE_OCI=1` 浏览器 E2E |
| **Release 幂等创建与 Git tag 发布** | `[x]` 本地隔离验收 | `(repo_id,version)` 请求摘要幂等；同版本不同请求冲突；tag 名仓库内唯一；发布用 compare-and-swap 固定 tag 到 commit。`releases_publish_http_e2e` 1/1、MCP 发布 E2E 1/1；migration `0048` 在完整 up/down/up 验收通过 |
| Compose **经产品 API** mock 回退 | `[x]` 显式拒绝 | `MUSEHUB_REQUIRE_PIPELINE_OCI=1` 时禁止 compose-mock / mock health；无 OCI 制品不得发布 |
| **生产 compose 远程 worker 全链** | `[x]` 本地 | `verify-product-production-stack-e2e.sh`；`install-production-compose.sh` |
| OCI / 多环境 / 业务 MCP | `[~]` | **52** 工具 + **23** 写幂等 + **5** §13 读；`writes_thin`（merge/respond/pipeline 幂等）+ `resources_v1_*`（含跨 org）；**无** OAuth 远程 |

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
- **Linux CI / 本地 Postgres**：`verify-product-acceptance.sh` 与 `musehub-migrate` 测试覆盖全量 migration；迁移文件当前为 0001–0048；2026-11-19 在本轮专用 PostgreSQL 17 数据库完成 0001–0048 完整 up/down/up，迁移验收 3/3 通过；OIDC 自助绑定 Playwright 1/1 通过（见 `docs/ACCEPTANCE-EVIDENCE.md`）。该脚本只接受专用数据库变量 `MUSEHUB_TEST_DATABASE_URL`，并要求显式设置 `MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1` 才会执行完整 down/up。`full_up_down` 的双独立连接池并发 up、空闲 peer 下 down、idle-peer no-op 于 2026-11-17 在一次性 PostgreSQL 17.7 数据库 3/3 通过。`0001_common.down.sql` 先按函数名显式拆除 `musehub_set_updated_at` 触发器再 `DROP FUNCTION`（不用 `CASCADE`）。可选 `MUSEHUB_MIGRATE_TERMINATE_IDLE_PEERS=1` 仅结束本库 **空闲** 客户端连接，缓解 acceptance 并发池化导致的 down 偶发失败，不默认在生产开启。
- **macOS 开发者机**（Homebrew PostgreSQL@17）：`musehub-migrate` 隔离集群测试仍只捆绑 `0001`+`0002` 子集（见 `up_down.rs`），与全量 acceptance 路径不同。
- `musehub-mcp-probe` 在 Linux 本地用 **两种独立 SDK 客户端**（`rust-rmcp` 3.8.0 + 官方 Python `mcp` 2.3.0）各覆盖 stdio 与 Streamable HTTP；见 `docs/m0-rmcp-matrix.md`。这是 **M0 互操作探针**（`m0_mcp_interop_matrix_passed` 由 `cargo test -p musehub-mcp-probe --test matrix` 验证），不等同于生产 API 验收。生产 `/mcp` 的工具授权、写工具幂等、资源读取和 Rust/Python Streamable HTTP 矩阵另由产品 E2E 与 `production_mcp_matrix` 验证；远程 OAuth 不在 V1 范围。

尚未完成，不能把 M0 标为通过：

- [x] 真实 Codex 适配器和显式在线验收脚本已经实现。2026-11-17 在线 AgentRun 验收通过：run `d9528061-40c5-46f3-9e39-197c3c230e40` 成功推送 commit `9ae13fd16876c84b769b33f3cc68140ea5fd345f`，proof 精确匹配；provider usage 为 input 77,467、output 340、total 77,807 tokens，与数据库记录一致，bare 分支指向该 commit。早期失败运行及旧 usage 快照不作为通过证据；token 阈值在 usage 报告后检查，不构成单回合硬上限。普通 CI 不调用在线模型。
- 产品 SSH Git 传输已实现 `AuthorizedKeysCommand`、gateway 强制命令和 `git_ssh_transport_e2e`；rootless/OpenSSH Colima 探针仍只证明 VM 内单用户场景。真实不可信贡献者的多租户隔离与安全边界仍需目标环境验收；物理多主机长期运行不属于 V1 Runner 要求。
- 同类产品完整对照尚未完成。2026-11-17 Forgejo 临时实例曾用 Forgejo Runner 11.0.0（固定摘要 `sha256:6e1cb83e64476a015c1182c39d5f9c0a5a62b22b826104a1b5ada5971246a6af`）执行简单 echo workflow；Runner 在内部 Docker 网络运行，限制 512 MiB/1 CPU/128 PIDs，不挂宿主 Docker socket，丢弃 Linux capabilities 并启用 `no-new-privileges`，但没有执行 Agent 或证明不可信代码隔离。OneDev 固定 digest 实例曾在服务容器内以临时 Host Executor 手动运行合成 Build；不代表独立 Runner、不验证自动触发或不可信代码隔离。2026-11-19 的同一 T1 fixture 基线 `e3949bea547a70a9346f36bfee1f9ce3d0d63edb` 与修复提交 `5447cb08bc42a2b5dc80715a35e8e65eb7dd4d88` 已推送到专用 Forgejo `t1admin/musehub-t1-pagination` 和 OneDev `musehub-t1-rerun`，并通过 `git ls-remote` 核对两边 OID 完全一致；Forgejo Issue #1/PR #2 与 OneDev Issue #4/PR #1 已创建。修复分支本地 `npm test` 为 3 pass、0 fail。随后 OneDev 17.2.0 build spec 更新为 version 53 / `!PosixInterpreter` 并配置 PullRequestUpdateTrigger。2026-11-19 Build #5 对源分支 commit `ec5c305f571e323bcbb15028e069746e71879cd3` 成功，Build #6 对 PR merge preview `f4fa70ec281c6461b99cba7043ce578d4a9711fc` 成功；两次均实际运行 `npm test`，3 pass、0 fail。Forgejo PR workflow 初次 run #1 及多次重跑先后暴露 Runner 外部 DNS、job 容器 Forgejo 主机名解析和 checkout action 缓存对象不完整问题；修复只作用于专用 `musehub-roadmap-*` Runner/网络/缓存。2026-11-19 Forgejo `t1-gitpat/t1-pagination` 的 PR #1 最终 Actions task #6 成功，实际运行 `npm test`，3 pass、0 fail；checkout action 的远程 `v4` 标签与提交 `11d5960a326750d5838078e36cf38b85af677262` 可达，损坏的专用缓存已隔离保留。共同 base 为 `e3949bea547a70a9346f36bfee1f9ce3d0d63edb`，Forgejo 修复 head 为 `ec5c305f571e323bcbb15028e069746e71879cd3`；OneDev Build #5/#6 对源分支和 PR merge preview 均真实运行 `npm test`，各 3 pass、0 fail。Forgejo 与 OneDev 的同起点 T1 PR CI 均已通过；完整三方试点流程、Agent 任务、人工操作/耗时记录及 T2–T10 横向对照仍未完成；详见 `docs/06-landscape-and-practice.md`。

**环境相关测试（本机证据，Linux CI 可能失败且未用 skip 掩盖）：** Colima/Homebrew 绑定的 `musehub-container-probe`、`musehub-rootless-probe`、`musehub-ssh-probe` 集成测试，以及 Homebrew PostgreSQL@17 上的 `musehub-migrate` 集成测试。若在 macOS 开发者机上跑全量 `cargo test --workspace`，需具备对应路径的二进制。
