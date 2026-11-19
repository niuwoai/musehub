# 同类产品、公开实践与 MuseHub 的取舍

> 调研日期：2026-10-30。来源以官方文档、官方代码仓库和官方支持案例为主。此次进行了公开资料核查，没有安装、购买或实测这些产品；以下“实践”指可查阅的教程/案例，不能当作本次实测报告。

## 1. 结论

已有多个产品覆盖“Git + CI/CD + AI Issue 实现 + PR + MCP”的大部分能力。MuseHub 面对的是已有产品的竞争，不是空白赛道。最值得优先体验的是 Gitea Enterprise Agents、OneDev AI User 和 GitLab Duo；Rust 方向另看 Mega 的设计。

需要验证的产品假设是：一个轻量、内网优先、Agent 可替换、证据链清楚、部署可恢复的平台，是否比现有产品更适合目标团队。Rust 是实现选择，不足以单独构成用户购买或迁移理由。

## 2. 对照表

“已文档化”表示官方有明确说明，不表示我们验收通过；没有查到的部分标注未核实，不能据此推断产品没有。

| 产品 | 与需求相近的能力 | 部署/版本边界 | 对 MuseHub 的主要价值 |
|---|---|---|---|
| GitLab + Duo Agent Platform | 研发平台、Issue→MR、Agent 流程、MCP | 官方列出 Self-Managed；模型、订阅与具体版本需核对 | 全生命周期模型、权限和企业治理 |
| Gitea Enterprise Agents | Agent 身份、Actions 执行、PR 维护、会话内 MCP | 当前文档说明 Enterprise v27+；不要混同社区版 | 最接近拟议架构的直接对标 |
| OneDev | Git/Issue/CI/CD，AI 用户处理 Issue/PR 和失败构建 | 具体 AI 权益与部署配置需按所选版本核验 | 紧凑工作流、失败修复闭环 |
| Forgejo | 自托管 Git 协作与 Actions | Actions 相似但不承诺 GitHub Actions 兼容 | 基础 forge 与独立 Runner 设计 |
| GitHub Copilot agents | 将 Issue 分配给 Agent，生成 PR 与迭代 | 此处核实的是 GitHub 云端流程，不能当作内网方案 | 任务入口与审查交互 |
| OpenHands / Agent Canvas | 自托管 Agent 控制台、多 Agent/后端适配 | 当前主仓 README 已以 Agent Canvas 为中心 | Agent 接入与隔离工作空间 |
| Coolify | Git/镜像触发应用部署、自托管容器应用 | 重点在应用部署，非本调研中的完整代码 forge | 从代码/镜像到环境的操作体验 |
| Mega | Rust、Git 兼容 monorepo、面向 Agent 的版本控制思路 | 本次打开的官方仓库显示已归档 | 源码架构研究；不直接视为持续维护底座 |

各行事实的来源及边界见下节。未对许可证兼容、采购价格或商用权益作法律/采购判断；采用代码或采购前分别核验具体 tag、LICENSE 和授权条款。

## 3. 优先看的产品

### 3.1 Gitea Enterprise Agents：最接近整体结构

官方说明 Agent 使用独立 bot 身份，通过 Actions Runner 执行，平台处理后续 PR 等动作；每次会话有内置 MCP，配置可版本化。该文档注明 Enterprise v27 及以上。[官方设计与操作说明](https://docs.gitea.com/enterprise/features/agents/)

建议阅读路径：How agents work → Credentials → Sessions → Built-in MCP → Delivery plans。研究时重点画清“执行代码”和“决定业务状态”的边界，并观察身份权限怎样绑定发起人。

对 MuseHub 的判断：AgentRun、短期委托权限、固定配置快照应成为首版基础设施；不能仅在 Issue 页面加一个聊天框。大需求多 PR 拆分可后置，先把单个任务的证据和恢复做好。

### 3.2 OneDev：最适合照着做一遍闭环

官方教程提供把 Issue 分配给 AI 用户，生成 PR，处理审查反馈及 CI/冲突的操作路径，并说明工作空间的创建与清理。[Issue 实现教程](https://docs.onedev.io/tutorials/ai/working-with-ai-user/collab-with-ai-user/assign-issues)

另一个教程展示构建失败后创建 Issue 并分配 AI 用户，还讨论避免同一失败链重复触发和 AI PR 的循环处理。[失败构建修复教程](https://docs.onedev.io/tutorials/ai/working-with-ai-user/collab-with-ai-user/fix-failed-builds)

对 MuseHub 的判断：自动修复最先需要事件去重和预算边界。首版先提供明确的“修复这次失败”操作，待积累真实任务结果后再启用规则自动触发。

### 3.3 GitLab Duo：完整研发流程的参照

官方列出 Agent Platform 的 Self-Managed 形态，覆盖多种 Agent 流程与 MCP；具体授权、模型和实例配置有前置要求。[平台能力与前置条件](https://docs.gitlab.com/user/duo_agent_platform/)

Developer Flow 文档展示从 Issue 创建 MR、根据反馈迭代等流程，并建议配置项目规则与执行环境。[Developer Flow 操作说明](https://docs.gitlab.com/user/project/merge_requests/developer/)

对 MuseHub 的判断：上下文与执行环境必须可复现；“模型接口能连通”只能作为连接测试。试用需要记录具体版本与功能状态，滚动文档中的新功能不能自动算入已安装版本。

### 3.4 Mega：Rust 方向的直接参考，注意维护状态

Mega 官方仓库将自己定位为面向 AI Agent 的 Rust Git 兼容 monorepo 引擎，并介绍与 Libra 的配合。此次访问页面显示仓库在 2026-10-19 归档；README 中还有路线图项目。[官方仓库](https://github.com/gitmono-dev/mega)

对 MuseHub 的判断：它适合研究 Git 服务、结构化意图与大型仓库的设计；其 monorepo/Piper 取向与 MuseHub 首版普通多仓库协作不同。不要把路线图、规模目标或 README 声明当作已验证性能；归档也不证明所有关联项目停止，需要分别核验。

## 4. 分模块参考

### Forgejo：Git forge 与流水线

Forgejo Actions 官方说明工作流交给独立 Runner 执行；它刻意区分“熟悉的 GitHub Actions 使用方式”和“兼容”。[Forgejo Actions 文档](https://forgejo.org/docs/latest/user/actions/overview/)

MuseHub 应借鉴这种明确的兼容边界。自有 YAML 能跑不是 GitHub Actions 全兼容，迁入工具只能处理经过验证的子集。

### OpenHands / Agent Canvas：Agent 可替换与后端分离

当前官方主仓 README 介绍自托管 Agent Canvas，可连接不同 Agent 与本地、容器或远程执行后端；它也指出共享目录仍会造成并行会话文件冲突。[官方仓库与运行方式](https://github.com/OpenHands/OpenHands)

MuseHub 可以研究其适配与执行生命周期，首版仍只承诺一个通过测试的组合。任务独立 worktree/clone、状态持久化和凭据隔离应由平台保证。

### GitHub Copilot agents：Issue 到可审查代码的入口

官方流程支持分配 Issue 给 Agent，让它产生 PR 并请求审查。[官方任务入口教程](https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/kick-off-a-task)

对 MuseHub 的参考点是减少发起操作、清晰展示审查接力。这里研究的是交互模式，不把 GitHub 云能力当作可私有部署组件。

### Coolify：Git/镜像到应用环境

官方应用文档覆盖 Git 仓库、Dockerfile、Compose 和现成镜像等部署来源；自动部署文档解释 Git 事件与认证部署请求触发。[应用模型](https://coolify.io/docs/applications/)；[自动部署](https://coolify.io/docs/applications/deployments/automatic-deployments)

MuseHub 应研究环境配置、部署日志和失败反馈的体验；自己的发布链需要额外坚持 commit→build→digest→observed runtime 的准确关联。

## 5. 一条具体的公开故障经验

GitLab 官方支持案例记录：不受支持的自托管模型可能创建分支/MR 后提前结束，实际没有 commit/push，结果出现空 MR；文字上的完成声明与 Git 状态不一致。[官方故障案例](https://support.gitlab.com/hc/en-us/articles/27951720216220-Duo-Developer-Flow-pushes-no-commits-when-using-an-unsupported-self-hosted-model)

MuseHub 的对应决策：模型“完成”事件触发平台核验；实现任务必须验证真实提交、远端 ref、diff 与验收证据。模型与 Agent 的兼容测试要覆盖工具调用、提交、推送、反馈迭代和取消，不能只测一次聊天请求。

## 6. 建议的一周体验计划（2026-11-17 未完成；仅有安装尝试）

使用新建的合成项目、独立 VM、临时凭据与明确模型费用上限。不要把真实生产私有仓库直接导入未知配置。安装前固定产品版本/镜像摘要，阅读该版本文档。

**2026-11-17 安装与任务尝试记录：** Forgejo 镜像 `codeberg.org/forgejo/forgejo@sha256:fa2b71d2ec931bd90b1744720af33ec4455a9d2e789591584830d7a7615ef87b` 在临时容器中返回 HTTP 200；已创建合成 Issue、推送 `main` 与 `acceptance` 分支、建立 PR，并让 PR 获得 1 个文件差异。随后在 `acceptance` 分支配置 `.forgejo/workflows/acceptance.yml`，Forgejo Runner 11.0.0 固定镜像在内部 Docker 网络中注册并运行 Actions Run 4，状态 `success`；工作流只有引用检查和 echo，不代表不可信代码隔离或通用 CI 构建能力。此前提交状态 API 查询为 0 个状态；未运行 Agent 任务。OneDev 镜像 `1dev/server@sha256:8eda046b8e24c024b6a2ba8d129302bc5c35ea3f744f340a7681a105be38aec4` 按官方容器指南挂载专用数据卷后 HTTP 返回 200；创建项目 `musehub-acceptance`（ID 1，key `MHA`）、Issue `MHA-1`，推送 `main` 与 `acceptance/change` 并创建 PR `MHA-1`（2 个文件差异）。为该合成项目配置临时 Server Host Executor，手动运行 `Acceptance Check`，Build #1 成功；命令在 OneDev 服务容器内执行，不构成独立 Runner或不可信代码隔离证据，也未配置自动触发或运行 Agent。两者统一 Issue/Git/PR/CI/Agent 对照仍未完成，当前证据不足以总结完整产品差异。

| 日程 | 工作 | 产出 |
|---|---|---|
| 第 1 天 | 读 Gitea Enterprise、OneDev、GitLab 对应教程；确认试用权益与可用版本 | 能力/版本/授权矩阵；选两个能实际安装的产品 |
| 第 2 天 | 创建同一小型仓库，验证用户、SSH/HTTPS、Issue、PR 和基础 CI | 安装时间、手工操作数、失败位置 |
| 第 3 天 | 同一 Issue 交给 Agent：小 bug、小功能、补测试 | 输入快照、commit、PR、费用、耗时、人工返工记录 |
| 第 4 天 | 故意制造失败 CI、审查反馈、变更 head、取消、服务重启 | 恢复行为、是否重复执行、旧审查是否失效 |
| 第 5 天 | 体验 Coolify 或目标平台的部署路径；发布、健康失败和回滚 | 来源与实际运行版本的追溯情况 |
| 第 6 天 | 阅读 Mega 与 OpenHands 的相应模块，不直接复用不明许可证代码 | 可借鉴的模块边界、维护状态、替代方案 |
| 第 7 天 | 汇总证据，对 MuseHub 首版范围做一次取舍 | 冻结 ADR、更新排期、给出继续/收缩/改集成方案判断 |

若企业版没有可用试用授权，标注“资料评估，未试用”，不为凑齐对比编造结果。首次体验无需同时安装全部产品。

### 统一任务集

T1 修复分页边界并添加回归测试；T2 新增一个小 API 和对应前端文案；T3 在已有 PR 上响应一条明确审查意见；T4 修复测试失败；T5 取消耗时任务；T6 新提交使旧批准失效；T7 发布固定制品并部署；T8 部署健康失败后回滚；T9 撤销发起人权限；T10 重放同一触发事件。

#### T1 跨产品可复现基准

以下是 Forgejo、OneDev 与 MuseHub 的共同首轮基准。2026-11-18 隔离 fixture 验证基准断言：固定的 1-based 错误实现使三项测试全部失败，按 `(page - 1) * pageSize` 修正后全通过；这只验证 oracle，不构成产品成绩。历史 OID `ea0cf80798c1c895e0c0d54e541275893f4ad13b` 无法从当前仓库或 GitHub 独立解析；确定性 fixture 的 source baseline 为 `e3949bea547a70a9346f36bfee1f9ce3d0d63edb`，独立修复 commit 为 `5447cb08bc42a2b5dc80715a35e8e65eb7dd4d88`。2026-11-19 MuseHub opt-in 产品 Runner T1 已通过：以该 source baseline 为共同起点，创建 Issue、错误基线和修复 PR；baseline run `9601b231-faff-4569-b156-136a9c96cff3` 为 failed，分页 job 预期 3 项失败；PR run `c539c3b3-afe0-4855-bedb-3b4576f6411d` 在 head `c2430bd4b0214f14b897ab414220b39d7e71a1b5` 上 3 pass、0 fail，`product-ci` 成功。该脚本使用 digest-pinned Node 镜像，并完成 Runner→DinD mTLS、artifact SHA-256 与缓存命中断言。早期 Forgejo 与 OneDev 记录使用无法核验的旧 base；2026-11-19 随后重建的专用实例均使用本段 source baseline，并在下文记录了真实 PR CI 结果。随后 MuseHub 隔离重跑通过：baseline run `2071bdf4-aaa8-4e5d-b280-0c10d3f9b13c` 三项失败，修复 PR run `93882a1a-ee69-4c40-ad41-b34779ae9f41` 在 head `021e7f0d47768946986572f6e4a3be0248de9b6c` 三项通过，`product-ci` 成功；共同 fixture base 仍为 `e3949bea547a70a9346f36bfee1f9ce3d0d63edb`。同轮 Runner mTLS、artifact SHA-256、缓存命中与全新数据库 migrations 0001–0046 完整 up/down/up 均通过。Forgejo `t1admin/musehub-t1-pagination` 与 OneDev `musehub-t1-rerun` 现均已核实 `main=e3949bea547a70a9346f36bfee1f9ce3d0d63edb`、`t1/pagination-fix=5447cb08bc42a2b5dc80715a35e8e65eb7dd4d88`；Forgejo 已创建 Issue #1、open PR #2，PR 指向上述共同提交；OneDev 已创建 Issue #4、open PR #1，来源/目标分支为 `t1/pagination-fix`→`main`。修复分支在本机 fixture 执行 `npm test` 为 3 pass、0 fail。随后 OneDev 17.2.0 build spec 更新为 version 53 / `!PosixInterpreter`，补齐必填字段并配置 PullRequestUpdateTrigger。2026-11-19 OneDev Build #5 对源分支 commit `ec5c305f571e323bcbb15028e069746e71879cd3` 成功，Build #6 对 PR merge preview `f4fa70ec281c6461b99cba7043ce578d4a9711fc` 成功；两次均实际运行 `npm test`，3 pass、0 fail。Forgejo PR workflow 初始失败与后续重跑先后暴露 Runner 外部 DNS、job 容器 Forgejo 主机名解析及 checkout action 缓存对象不完整问题。只修改专用 Runner 的外部 DNS/主机映射，并隔离保留损坏的专用缓存后，2026-11-19 Forgejo `t1-gitpat/t1-pagination` PR #1 的 Actions task #6 成功完成 checkout，实际运行 `npm test`，3 pass、0 fail；远程 `actions/checkout@v4` 标签及提交 `11d5960a326750d5838078e36cf38b85af677262` 可达。Forgejo 共同 base 为 `e3949bea547a70a9346f36bfee1f9ce3d0d63edb`，修复 head 为 `ec5c305f571e323bcbb15028e069746e71879cd3`。OneDev Build #5/#6 对源分支和 PR merge preview 均真实执行测试并通过（各 3 pass、0 fail）。因此 Forgejo 与 OneDev 的同起点 T1 PR CI 均已通过；双产品更广泛对照、Agent 任务、安装/人工操作耗时仍未完成。

- **Issue 标题：** `Fix 1-based pagination boundary and add regression tests`
- **Issue 正文：** `paginate(items, page, pageSize) currently skips the first page because page is 1-based but the slice offset is 0-based. Make page=1 return the first page, preserve the final partial page, and add regression tests for page 1, page 2, and the final partial page. Do not change unrelated files.`
- **固定起始实现：** JavaScript `paginate(items, page, pageSize)` 使用 `items.slice(page * pageSize, (page + 1) * pageSize)`；仓库仅含该函数、Node.js 内置 `node:test` 回归测试和 `npm test` 脚本，避免引入第三方依赖。
- **通过条件：** page 1 返回首段，page 2 返回第二段，最后一页保留不足 `pageSize` 的剩余项；所有测试通过；同一个任务分支上有新 commit；PR 指向该 commit；CI 对该 PR/head 成功。记录每项人工操作、是否自动触发 CI、从 Issue 到 CI 的耗时。Agent 是否可用、费用和返工单独记为产品能力，不作为共同基准的隐含前提。
- **对照规则：** 给每个系统导入完全相同的 Git 起始 commit 和 Issue 正文，使用相同镜像/产品版本记录；不要复用前一轮不同内容的 issue、分支、diff 或手动 Build 作为 T1 成绩。若环境或授权不支持某环节，记为 `blocked` / `not tested`。

**既有试用记录与本轮结果的边界：** 2026-11-17 的 Forgejo 与 OneDev 都完成了基本 Issue/Git/PR/CI 操作，但没有共同的需求 hash 或共同起始 commit；Forgejo PR 有 1 个文件差异并由 Runner 自动执行合成 workflow，OneDev PR 有 2 个文件差异且通过服务容器内 Host Executor 手动 Build。它们不能横向比较耗时、操作数、CI 隔离或 Agent 完成率。本轮 2026-11-18 在专用实例和 fixture 上执行了 T1：MuseHub 建立了任务 Issue、PR 与 validation run，但该 run 未执行仓库测试；Forgejo 专用实例 `t1admin/musehub-t1-20261118-r3` 建立 T1 Issue #2、PR #2，并由专用 Forgejo Runner 对 PR head `500444677148e30aba299af4da5650992fcf8e62` 自动运行 `npm test`，3 pass、0 fail；OneDev 在专用项目 `musehub-t1-20261118-r4` 建立 Issue #2 与 PR #1，Build #1 因 build spec 未检出代码而失败，随后同一任务分支加入 OneDev checkout step 并提交 `4bd42cfc35ca4dceff014f80189baf5b8b17a1e3`；Build #2 成功并实际运行 `npm test`（3 pass、0 fail），PR updates API 记录了该 head。Forgejo/OneDev 专用实例均返回 HTTP 200；OneDev 临时管理员密码由运行时随机生成且未输出，Build 脚本在退出时尝试删除临时 API token。Forgejo Runner 及其状态卷为专用测试资源；Forgejo 专用实例中现有 13 条本轮生成的临时 token 元数据，API 撤销未完成，未输出 token 值，也未重置管理员密码。未记录可核验的 Issue 到 CI 总耗时；MuseHub validation 与 OneDev Build 均由显式 API 调用发起，Forgejo CI 由 PR 事件自动触发。上述 2026-11-18 结果不是统一任务的同一 Git 起点，不能作为三方对照结论。2026-11-19 同起点 T1 PR CI 后续已在「T1 跨产品可复现基准」记录：Forgejo 与 OneDev 均真实运行 `npm test` 并各 3 pass、0 fail；完整三方任务操作记录、Agent 任务与耗时仍未完成。此前 13 条临时 token 元数据已从专用测试账户撤销。

### PILOT-01：固定 20 项 AI 质量评估

本评估独立于上面的 T1–T10 产品对照任务。`scripts/pilot_report.py` 提供离线清单冻结与报告生成，不调用模型、API 或数据库，也不生成试点成绩；对应的 11 项契约单测已纳入 portable CI，并在本机全数通过。

先准备一个 catalog JSON：`schema_version` 为 `1`、`task_set_id` 唯一，`tasks` 恰好 20 项；每项包含唯一 `task_id`、类别 `bugfix` / `feature` / `test`、试点仓库别名 `pilot_repo` 和任务输入 UTF-8 原文的 SHA-256 `prompt_sha256`。清单必须覆盖三种类别和恰好两个仓库别名。冻结后把 `catalog_sha256` 单独记录并用于所有结果文件；两仓的真实名称与别名映射留在受控环境，不放进脱敏报告。

```bash
python3 scripts/pilot_report.py freeze pilot-catalog.json pilot-catalog.lock.json
python3 scripts/pilot_report.py template pilot-catalog.lock.json pilot-results.json
# 在真实执行后填写 pilot-results.json，再生成报告
python3 scripts/pilot_report.py report pilot-catalog.lock.json pilot-results.json --json pilot-report.json --markdown pilot-report.md
```

结果 JSON 必须引用冻结清单 hash，含 MuseHub 版本、Agent/model 标签、代码 commit、环境标签、带时区的起止时间，以及恰好 20 条唯一任务记录。每条记录含状态（`completed` / `failed` / `blocked` / `not_tested`）、PR 数字 ID、commit OID、CI 状态、人工接受标记、返工分钟数、USD 费用字符串和费用估算标记；缺失的 PR/commit、返工或费用用 `null`，不得补造。结果还必须包含 `defects` 数组（可为空），每项关联清单中的任务并记录唯一缺陷 ID、严重级别、摘要和证据引用；以及 `release_decision` 对象，记录人工决定（`continue` / `pause` / `stop`）、审核标签、带时区的决定时间和非空理由。决定时间不得早于试点结束时间。

报告将完成率和有效 PR 比例均以 20 项为分母。有效 PR 定义为状态 `completed`、人工接受、具有 PR ID 与 commit OID、且 CI `passed`；返工报告已记录项总分钟数/平均分钟数及覆盖数，费用报告已知总额、估算总额和覆盖数，并按任务类别与两仓别名拆分完成数和有效 PR 数。报告同时列出人工缺陷清单和发布判断，不设成功率门槛，也不自动推断发布结论。输入校验失败时返回非零错误；冻结清单与报告输出均拒绝覆盖现有文件。

真实 20 项任务的内容冻结、两个试点仓库的执行、人工返工/费用记录、缺陷清单及发布判断仍待完成；本地验证数据不得作为真实试点成绩。

### 记录模板

```text
产品/版本/镜像摘要：
部署拓扑、模型和 Agent 版本：
任务编号与输入需求 hash：
开始/结束时间、总费用/估计费用标识：
Issue → Run → Commit → PR → Pipeline → Release → Deployment 链接：
结果：通过 / 失败 / 阻塞 / 未测试
人工操作次数与返工分钟数：
故障注入与恢复结果：
截图/日志/命令报告位置（脱敏）：
版本/授权限制：
值得借鉴的行为与不适用原因：
```

## 7. 对 MuseHub 的竞争假设

| 假设 | 怎么验证 | 不成立时怎么办 |
|---|---|---|
| 安装与日常维护比大型平台更轻 | 在同样机器记录安装步骤、资源占用、升级/恢复耗时 | 缩减必需组件；不以 Rust 语言推断性能 |
| Agent 替换成本低 | 第二个适配器只修改 adapter 层，不改 Issue/PR/流水线领域逻辑 | 调整契约，减少对特定 CLI 事件的依赖 |
| 证据链减少返工 | 同组任务比较定位失败与验证结果的时间 | 优先完善领域关联与 UI，暂缓更多 AI 功能 |
| 内网边界更可控 | 禁公网环境下运行预置依赖/模型任务，记录所有出口 | 补齐离线依赖与安装检查，明确不支持项 |
| 用户愿意迁入新 forge | 两个试点仓库持续使用且维护者认可迁移成本 | 评估先做现有 forge 上层交付服务，保留独立 Git 目标作为后续 |

本次没有足够证据给这些产品做可靠性排名，也没有证据证明 MuseHub 会比它们更快或更便宜。下一步应以同一任务和故障场景的试验结果决定投入。
