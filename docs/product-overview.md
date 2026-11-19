# MuseHub 产品概览

> 更新：2026-11-19｜文档基线：0.1｜开发版本：0.20.0-rc6｜阶段：M0 进行中，尚未发布。当前仍缺真实公网 TLS、GitHub OAuth App callback、物理双机/多区域故障切换及完整三方试点评估；API 与 Runner 调度性能测量脚本已提供，但尚无真实运行结果。

## 定位

面向需要掌握代码、执行环境和模型访问边界的研发团队，提供可私有部署的 Git 协作与 AI 软件交付平台。首批目标是愿意参与试用的 5–50 人团队；这是假设客群，不是已验证的市场结论。

## 核心对象

Repository 管代码；Issue 定义问题与验收；Plan 保存实现方案；AgentRun 记录一次执行；PR 组织审查；PipelineRun 记录验证；Release 固定发布内容；Deployment 记录实际环境结果。

每个对象都具有稳定 ID、状态、责任主体和可追溯关系。需求实现完成、发布完成、部署健康是三个不同结果，界面分别显示。

## 首版范围

1. 内网用户、组织创建与列表、工作区切换、角色、PAT、SSH key 和审计。
2. 私有 Git 仓库、SSH/HTTPS clone/fetch/push、文件与 diff 浏览、同仓库分支 PR、保护分支。
3. Issue、不可变需求版本、验收条目、实现计划、讨论和交付关联。
4. 一个可实测的编码 Agent 适配器，任务排队、取消、预算、失败说明、反馈后新一轮执行。
5. YAML DAG 流水线、独立 Linux Runner、容器执行、缓存、制品、日志、重试与取消。
6. 人工审查、检查门禁、合并后构建、固定 tag/commit 的 Release。
7. Docker Compose 测试环境部署、健康检查、受保护环境批准、回滚到保留版本。
8. REST 与 MCP 覆盖读取、Issue、PR、任务、流水线、发布和部署。

## 明确后续处理

Git LFS、跨仓库 fork PR、持续镜像、代码语义索引、原生 K8s/Helm、完整 GitHub Actions 兼容、Windows/macOS Runner、通用软件包注册表、复杂多 Agent 协作，进入后续版本。共享文件系统挂载与双主机可见性/锁/幸存节点验收脚本已提供；外部 PostgreSQL HA endpoint、primary/standby 检查和 standby 写请求门禁也已实现。CloudNativePG 三实例单区域 HA 参考清单已在临时 k3d 集群部署并验收 Pod 与 worker 容器故障后的自动提升，已确认同步提交仍可读取；另新增 Barman Cloud 异地备库配置和基于旧主 demotionToken 的受控提升脚本。k3d 实验使用本机 Docker 与 local-path 存储，不验证物理节点故障、网络分区 fencing、云存储耐久性、生产 writer/API 流量切换或多区域恢复。多区域 PostgreSQL 配置与操作流程已提供，但尚未在独立 region 部署验收；另新增 region A→B 的 SSH/rsync 异步单向 Git/制品副本及专用 systemd 安装脚本；2026-11-17 在隔离 Colima VM 中验收真实 mirror service/timer、本地同步和 guest reboot 后再次运行。真实双机远端复制延迟、RPO、停机收敛和故障提升仍需在目标基础设施验收。共享 POSIX 挂载与云端存储服务仍由运营方提供。切换后检查脚本可验证旧主不可写和所有 API 实例状态。首版是端到端可试用版本，不等同于全面替代 GitHub/GitLab。

## 用户界面

以仓库为工作入口：代码、Issues、PR、流水线、Agent 执行、Releases、环境、设置。Issue/PR 详情内显示交付时间线；全局工作台突出等待审查、等待输入、失败任务和部署异常。工具型密集布局，中英文文案均从 i18n 资源读取。

## 运行形态

首版推荐安装包包含服务端、PostgreSQL、Git 工具和备份命令，Runner 放在独立机器或 VM。OCI Registry、模型端点、企业反向代理是外部集成。完全断网模式依赖预置的模型、Agent、镜像和依赖镜像，不会因“私有部署”自动获得。

## 成功标准

小型试点项目完成真实 Issue 到测试环境交付；能验证授权、故障恢复、幂等、防止旧测试结果错误放行；重启不会丢失已接受任务；每次部署可追溯并可检查回滚结果。AI 输出质量另用固定任务集评估，不能用流水线通过率替代业务正确性。

## 当前实现

**产品路径（Linux CI / Compose）：** 账号与组织创建/列表/切换、Git HTTP 与固定提交的代码浏览（可分页提交历史、目录面包屑、相对父提交的文件 diff）、Issue/计划、参考 Agent 与 codex-sim、PR/流水线/Release/Compose 部署、备份恢复、产品 MCP 与多数 REST 已在 acceptance 与安全 E2E 中验证；`repo-code-console` Playwright 已在本机 PostgreSQL 17 开发库通过，覆盖提交历史分页（首屏 20 条、追加 2 条、无重复且末页隐藏加载按钮）、文本 diff 与含空格的文件路径；AgentRun 启动时由服务端在同一事务内锁定当前 Issue 修订版、验收标准与已接受计划，并将快照写入任务输入，REST 与 MCP 行为一致；Issue 页面支持编辑并接受计划，计划未接受或内容尚未接受时不能启动 AgentRun；`issue-plan-acceptance` Playwright 已在本地开发栈通过（1 passed），覆盖计划首次接受、编辑后的启动禁用及新计划重新接受，`plan_immutable` REST 验收检查了 AgentRun 持久化的修订号、验收标准与已接受计划快照。组织创建与授权由 `organizations_http_e2e` 覆盖。单 Provider OIDC Authorization Code + PKCE 登录已实现，支持用户在账户安全页把经验证的 `(issuer, subject)` 身份绑定到自身账号；2026-11-17 在隔离 Keycloak 26.4.2 上完成真实浏览器授权码登录，验证未绑定身份拒绝、绑定后建立 MuseHub 会话并写入登录审计。2026-11-19 使用同一固定版本 Keycloak 与专用 PostgreSQL 17 完成账户安全页 OIDC 自助绑定浏览器验收（1/1），验证授权回调 303 与绑定状态；migration 0001–0047 完成 up/down/up。真实企业 IdP 仍未验收。另提供组织范围的 SCIM Users/Groups 接口子集，支持 token 管理、Users 与 Groups 增删改查及 PATCH；Users 支持 userName、externalId、displayName、active 等值过滤和白名单排序，Groups 支持 displayName、externalId 等值过滤和白名单排序。Group 成员必须属于同一组织的 SCIM Users，成员关系仅作为目录数据保存，不授予 MuseHub 权限；支持有界 SCIM Bulk 子集（最多 100 项、1 MiB；Users/Groups 的创建、替换、PATCH、删除按序逐项执行并回传结果；支持引用同批次内先前成功创建的资源，不支持前向引用）；不支持密码同步，也不会按邮箱自动认领身份。实验性 SAML SP 发起登录要求 IdP 元数据由固定 PEM 证书验证签名，使用 HTTPS ACS、数据库一次性请求关联和持久化重放防护，并且只接受管理员预先绑定的 `(issuer, NameID)`。SAML crate 尚未到 1.0 且未声明外部安全审计；2026-11-17 已在隔离 Keycloak 26.4.2 上完成 SP 发起的本地浏览器流程，POST ACS 返回 303、MuseHub 会话建立并由 `/api/v1/me` 返回 200。Keycloak 原生 realm metadata 未签名，因此本次为满足固定证书验签而使用补根 ID、由临时测试证书签名且 `WantAuthnRequestsSigned=false` 的合成 metadata fixture，并将 Keycloak client 配为不要求 AuthnRequest 签名；真实企业 IdP 互操作按 V1 范围延期。GitHub OAuth App 登录现已实现：使用 GitHub 不变数字 ID 绑定身份，不按邮箱自动关联；首次登录可 provision 独立 human principal，不自动加入组织。真实 App 凭据和线上 callback 尚未验收。SCIM 新建账号需另行建立可用的登录身份。单机安装见 README「自托管安装」与 `deploy/RUNBOOK.md`。**仍未验收：** GitHub OAuth App 真实凭据与线上 callback、真实企业 IdP 联调（已延期）、真实公网 TLS 环境（验收脚本已提供，尚未运行）、物理双机长期运行（已有实验性 Compose 模板及 PostgreSQL 仓库写锁；多主机争用与长期运行尚未验收）。Live Codex 在线成功提交已于 2026-11-17 验收通过，run、commit 与 usage 证据见 [docs/ACCEPTANCE-EVIDENCE.md](ACCEPTANCE-EVIDENCE.md)。多区域 PostgreSQL DR 配置与受控提升流程已有参考实现；另新增 PostgreSQL/storage recovery barrier，将已确认静默后的 WAL LSN 与最终 Git/制品镜像状态绑定，并已在单容器 PostgreSQL 主备模拟中验证追平门禁和提升后检查。该演练不验证真实跨区部署、远端复制延迟/RPO、fencing 或 API 切流，这些仍未验收。**探针层（M0）：** Colima digest/rootless/SSH、rmcp 互操作探针（`musehub-mcp-probe`）与 Agent fixture — 不代表生产能力。

## 当前待办

- 通过首轮技术验证选定一个 Agent 适配器和实测模型组合。
- 验证 rootless 隔离构建，以及按 OCI digest 部署 Compose。
- ~~用两个客户端验证 rmcp 的 stdio/HTTP 接入。~~ **M0 探针已覆盖**（`musehub-mcp-probe`）；生产 MCP 工具目录、仓库历史读取、资源授权与写操作幂等已有实现和 acceptance 证明。生产 `/mcp` 的 Rust 与 Python SDK Streamable HTTP 兼容矩阵见 [docs/mcp-compatibility-matrix.md](mcp-compatibility-matrix.md)。OAuth 远程授权不属于 V1 承诺。
- 以同一组任务体验 Gitea Enterprise、OneDev、GitLab Duo；2026-11-17 的 Forgejo 与 OneDev（补齐官方要求的 `/opt/onedev` 数据卷后）均已返回 HTTP 200；目前只确认服务可访问，统一 Issue/Git/PR 任务、GitLab/Gitea 企业版试用及差异结论仍未完成。
- 冻结初始仓库规模、部署主机条件、数据保留期与许可证。
- 完成 M0 后再承诺工期和性能指标。
