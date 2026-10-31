# MuseHub 开发与验证计划

> 2026-10-30｜规划基线 0.1。设计文档之外，BOOT-01 和 BOOT-02 已有实现与验证；其余工作包、工期和验收结果均不是已完成事项。

## 1. 组织方式与估算前提

以 3 名全职工程师为估算假设：Rust/Git/数据、Runner/交付、前端/全栈各一人；产品决策、测试、安全与运维需要部分工时支持。可以使用 AI 辅助编码，但不假设它消除协议、权限和故障恢复验证的工作。

M0–M6 按下表约 20–27 周的基础排程，加入 25%–30% 不确定性后建议按 25–35 周规划，约 75–105 工程人周。不是交付承诺；M0 后依据实际 Agent/Git/部署验证重估。单人按相同范围推进，应按约 18–24 个月量级评估，缩小范围才可能明显提前。

先做端到端骨架并持续验证，不等所有 Git UI 完成才接 Agent。P0 门禁需要完整通过，若预算不足应明确发布为更窄的技术预览，不能把未完成项从验收表静默移除。

## 2. 里程碑

| 阶段 | 基础工期 | 主要成果 | 退出条件 |
|---|---|---|---|
| M0 风险验证 | 1–2 周 | Git 协议探针、Runner 隔离、Agent 适配器选型、竞品试用、MCP 兼容矩阵 | clone/push、Agent 非空提交、取消、镜像 digest 部署均有最小证据；选择 ADR 固定 |
| M1 Git 与身份 | 4–5 周 | Rust workspace、安装、账号/PAT/SSH、仓库、权限、版本、审计 | IAM-01/02/03、GIT-01/02/04 关键路径；鉴权负向测试通过 |
| M2 Issue/PR 协作 | 3–4 周 | Issue 快照/计划、PR diff/评论、同仓库 merge、基础 MCP | ISS-01/02、PR-01；旧 head 审查失效；无需模型的人工协作可用 |
| M3 持久 CI | 4–5 周 | DAG、Runner、lease/fencing、日志/缓存/制品、合并候选 checks | CI-01/02/03、PR-02；重启/重复投递/过期结果实验通过 |
| M4 Agent 闭环 | 3–4 周 | 一个真实适配器、权限委托、预算、输入等待、反馈、结果验证 | AGT-01/02/03；真实 Issue→非空 PR；确认 canceled 后无新副作用，失联不误报已取消 |
| M5 Release/部署 | 3–4 周 | 固定制品 Release、Compose、审批、健康、回滚、交付时间线 | REL-01、DEP-01/02；失败与未知结果可恢复；完整主线通过 |
| M6 试点与加固 | 2–3 周 | 迁入、备份恢复、容量/安全/可用性验证、中英文、文档 | GIT-03/05、MCP-01、OPS-01、UI-01 与所有 P0 回归通过 |

M1 起每个领域同时提供 REST、Web 必要操作与 MCP 适配，不把整个 MCP 留到最后。M3 前的 PR 合并仅限开发环境，P0 分支门禁完整前不能开放受保护生产使用。

## 3. 工作包及依赖

| ID | 工作包 | 依赖 | 交付证据 |
|---|---|---|---|
| BOOT-01 | workspace、配置、错误模型、版本源、日志、最小 Web | 无 | cargo/yarn 构建、版本一致检查 |
| BOOT-02 | PostgreSQL migration 管理与隔离测试库 | BOOT-01 | 已完成：`musehub-migrate` 在隔离 PostgreSQL 17 上对 `0001_common`、`0002_identity` 执行 up/down/up，并检查时间戳与外键索引。其余表未落地 |
| AUTH-01 | principal/org/membership、登录与会话 | BOOT-02 | 角色/禁用/CSRF 用例 |
| AUTH-02 | PAT、SSH key、scope、审计 | AUTH-01 | token 撤销与跨租户测试 |
| GIT-IMP-01 | 仓库创建、受控 Git 子进程、HTTPS | AUTH-02 | 标准客户端 clone/fetch/push |
| GIT-IMP-02 | SSH forced command、统一 hook、写 intent | GIT-IMP-01 | 直推/force/delete 绕过测试、崩溃对账 |
| COLLAB-01 | Issue、Revision、Criteria、Plan、讨论 | AUTH-01 | 旧任务引用不可变输入 |
| COLLAB-02 | PR diff/revision/review、候选 merge、CAS | GIT-IMP-02、COLLAB-01 | 并发 head/base 变化测试 |
| RUN-01 | job/attempt/lease、领取、心跳、取消、outbox | BOOT-02、AUTH-02 | kill/restart/分区/迟到结果报告 |
| RUN-02 | rootless 隔离、资源/网络策略、secret 注入 | RUN-01 | 工作空间越界与 secret 边界测试 |
| CI-IMP-01 | YAML 解析、DAG、触发与 trust class | RUN-01、GIT-IMP-02 | DAG/配置篡改/重复事件测试 |
| CI-IMP-02 | logs、artifacts、cache、reports、BuildKit | RUN-02、CI-IMP-01 | hash/缓存隔离/容量边界测试 |
| MERGE-01 | 候选 checks、独立审查、精确 commit 合并 | COLLAB-02、CI-IMP-02 | 旧检查无法合并，实际 M 与测试 M 相同 |
| AGENT-IMP-01 | 适配器契约、profile、冻结上下文 | COLLAB-01、RUN-02 | 契约夹具与一个真实 CLI/Agent 集成 |
| AGENT-IMP-02 | 输出核验、PR 幂等、反馈/等待/预算 | AGENT-IMP-01、MERGE-01 | 真实小任务、空提交拒绝、权限中途撤销 |
| REL-IMP-01 | Release manifest、固定 tag、普通资产与 OCI | CI-IMP-02、MERGE-01 | tag/DB 故障窗口、资产不一致拒绝 |
| DEP-IMP-01 | 环境配置版本、审批、Compose、观察、回滚 | REL-IMP-01、RUN-02 | 健康失败/unknown/并发部署实验 |
| TRACE-01 | Issue→执行→PR→Release→环境时间线 | AGENT-IMP-02、DEP-IMP-01 | 正反向追溯与权限过滤 |
| MCP-IMP-01 | rmcp、stdio/HTTP、统一 schema/错误/幂等 | BOOT-01、AUTH-02 | 工具调用契约和客户端矩阵 |
| MCP-IMP-02 | 各领域工具、resources、prompts | 对应领域实现 | 工具目录全覆盖与越权回归 |
| OPS-IMP-01 | 安装、升级、备份、恢复、监控、保留策略 | 所有领域存储稳定 | 全新 VM 安装及恢复演练 |
| PILOT-01 | 20 个固定任务与两个试点仓库 | 全部 P0 | JSON/Markdown 报告、缺陷清单、发布判断 |

三条工程工作线可以按依赖并行推进，这是后续团队计划，不代表本次文档由多个代理执行。

## 4. M0 必须做出的选择

1. 用 Linux 验证 OpenSSH + forced command + Rust gateway 的身份传递、并发与取消。Colima Linux VM 内的单用户临时 sshd 已给出这项证据；它不传输真实 Git pack，也不是生产 Git 服务。
2. 选择 Agent：以 headless 运行、结构化输出、可配置端点、工具权限、取消、预算观测和许可证为筛选条件。最终至少一个组合通过真实任务；尚未指定某个品牌默认获胜。
3. 验证隔离构建与 rootless 镜像构建，不把宿主 Docker socket 暴露给 PR/Agent。
4. 验证一个 OCI 镜像按 digest 部署 Compose，并确认实际运行摘要。
5. 通过两个客户端检查 rmcp 协议和 stdio/HTTP 接入；需要 OAuth 的客户端单独记录为后续。
6. 跑同类产品体验中的最小任务集，确认 MuseHub 值得优先解决的差异。

任何一项失败都应修改设计或范围后再开工大规模实现，不能用 mock 演示替代可行性证据。

## 5. 测试分层

| 层 | 工具/方式 | 必测对象 |
|---|---|---|
| 单元 | cargo test | 状态机、策略、schema、DAG、预算、幂等 hash |
| 数据库集成 | 临时 PostgreSQL 容器/隔离数据库 | 约束、migration、竞争、租约、outbox |
| Git 协议 | 标准 Git 客户端 + Ruby 编排 | HTTPS/SSH、空仓库、大 pack、断线、force/delete、ref CAS |
| Runner 集成 | Linux VM + rootless 容器 | 资源限制、网络策略、凭据生命周期、工作空间清理 |
| 故障恢复 | 可重复故障注入脚本 | 服务 kill、Runner 失联、迟到回报、重复事件、部署 unknown |
| 前端 | Vitest + Testing Library + Playwright | 人工主线、Agent 主线、PR 审查、取消/失败、i18n、键盘操作 |
| MCP 契约 | Rust 客户端夹具 + 实际客户端 | schema、错误、分页、授权、幂等、版本协商 |
| AI 效果 | 冻结任务 + 真实 Agent/模型 | 有效提交、业务验收、返工、费用、成功/失败原因 |

测试脚本优先 Ruby，输出 `.json` 原始结果与 `.md` 人类报告，附版本、commit、环境和时间；证据里不得含凭据。真实模型测试必须有显式预算配置，普通 CI 用本地 fixture，不自动产生模型费用。

## 6. 每阶段完成定义

- 需求编号与自动化/人工验收用例对应；功能失败时有可定位错误。
- Rust `cargo fmt --check`、`cargo clippy`、相关 `cargo test` 通过；涉及全局变更运行 workspace 回归。
- Web 使用 yarn 执行类型检查、Vitest、构建、i18n 严格扫描；关键流程 Playwright。
- migration 可新库安装、回滚演练；不存在无 created_at/updated_at 的业务表。
- REST/MCP schema 与文档一致；版本、Changelog、产品概览同步。
- 没有未经授权的生产变更；没有 secret 进入源码、日志或报告。
- 任何性能声明均附测量条件，未测试项目明确标注，不由模型总结补全。

## 7. 发布门禁

`0.1.0-rc1` 是内部试点候选，必须完成全部 P0 主线。Git 数据损坏、越权、保护分支绕过、旧 CI 错误放行、取消后仍有不受控发布、部署结果误报、备份不可恢复均为阻断项。

建议分阶段版本：实现期间 `0.1.0-rcN` 递增；基线功能冻结且试点验收后再决定 `0.1.0`。修复只增加 rc，新增正式功能按语义版本调整。workspace/package/server header 从单一版本源校验同步。

首版交付物：版本化安装制品、配置示例、管理员手册、备份恢复手册、Git/MCP 兼容矩阵、API 契约、迁入步骤、已知限制、完整验收报告。是否开源和许可证由项目所有者决定，不预填许可证声明。

## 8. 最大风险及调整方式

| 风险 | 早期信号 | 处理 |
|---|---|---|
| 平台范围过大 | 连续阶段只补基础 UI，闭环无法演示 | 保留主线，后置高级 Git/Actions/多云能力 |
| Git 一致性错误 | ref 更新成功但业务丢事件，或并发保护失效 | M0/M1 优先验证 intent、锁、CAS、hook 与对账 |
| Agent 不可靠 | 空 PR、声称成功但没提交、费用失控 | 平台核验结果、能力声明、预算、人工接手 |
| Runner 泄露凭据 | PR 能读取发布 secret 或 host socket | 信任域拆分、最小令牌、VM/主机隔离 |
| 回滚误解 | 应用回滚后数据库不兼容 | 发布前明确迁移条件，首版不做自动数据库回滚 |
| 产品差异不足 | 试点认为现有产品已经足够 | 以体验数据重估，考虑先集中交付层集成 |

## 9. 下一次开工的明确入口

先执行 M0，建立独立 Rust workspace 与最小 Git/Runner/Agent 探针；不要同时铺完所有数据库表和页面。完成可行性报告后冻结适配器与接口，再按 M1 开始正式实现。本次请求只交付文档，不自动执行安装竞品、模型调用、代码实现或发布。
