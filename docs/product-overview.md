# MuseHub 产品概览

> 更新：2026-11-08｜文档基线：0.1｜开发版本：0.1.0-rc2｜阶段：M0 进行中，尚未发布。

## 定位

面向需要掌握代码、执行环境和模型访问边界的研发团队，提供可私有部署的 Git 协作与 AI 软件交付平台。首批目标是愿意参与试用的 5–50 人团队；这是假设客群，不是已验证的市场结论。

## 核心对象

Repository 管代码；Issue 定义问题与验收；Plan 保存实现方案；AgentRun 记录一次执行；PR 组织审查；PipelineRun 记录验证；Release 固定发布内容；Deployment 记录实际环境结果。

每个对象都具有稳定 ID、状态、责任主体和可追溯关系。需求实现完成、发布完成、部署健康是三个不同结果，界面分别显示。

## 首版范围

1. 内网用户、组织、角色、PAT、SSH key 和审计。
2. 私有 Git 仓库、SSH/HTTPS clone/fetch/push、文件与 diff 浏览、同仓库分支 PR、保护分支。
3. Issue、不可变需求版本、验收条目、实现计划、讨论和交付关联。
4. 一个可实测的编码 Agent 适配器，任务排队、取消、预算、失败说明、反馈后新一轮执行。
5. YAML DAG 流水线、独立 Linux Runner、容器执行、缓存、制品、日志、重试与取消。
6. 人工审查、检查门禁、合并后构建、固定 tag/commit 的 Release。
7. Docker Compose 测试环境部署、健康检查、受保护环境批准、回滚到保留版本。
8. REST 与 MCP 覆盖读取、Issue、PR、任务、流水线、发布和部署。

## 明确后续处理

Git LFS、跨仓库 fork PR、持续镜像、代码语义索引、原生 K8s/Helm、完整 GitHub Actions 兼容、Windows/macOS Runner、多地域 HA、通用软件包注册表、复杂多 Agent 协作，进入后续版本。首版是端到端可试用版本，不等同于全面替代 GitHub/GitLab。

## 用户界面

以仓库为工作入口：代码、Issues、PR、流水线、Agent 执行、Releases、环境、设置。Issue/PR 详情内显示交付时间线；全局工作台突出等待审查、等待输入、失败任务和部署异常。工具型密集布局，中英文文案均从 i18n 资源读取。

## 运行形态

首版推荐安装包包含服务端、PostgreSQL、Git 工具和备份命令，Runner 放在独立机器或 VM。OCI Registry、模型端点、企业反向代理是外部集成。完全断网模式依赖预置的模型、Agent、镜像和依赖镜像，不会因“私有部署”自动获得。

## 成功标准

小型试点项目完成真实 Issue 到测试环境交付；能验证授权、故障恢复、幂等、防止旧测试结果错误放行；重启不会丢失已接受任务；每次部署可追溯并可检查回滚结果。AI 输出质量另用固定任务集评估，不能用流水线通过率替代业务正确性。

## 当前实现

已完成最小服务、单一版本源、本地 Git 探针、本地进程探针、Agent fixture、rmcp M0 互操作探针（stdio + Streamable HTTP，见 `docs/m0-rmcp-matrix.md`）、Colima Docker digest 探针、Colima Linux VM 内的单用户 rootless 探针、Colima Linux VM 内的单用户 OpenSSH forced command 探针和最小 Web。还没有用户、仓库托管、Issue、PR、流水线、真实 Agent、Release、生产 MCP 或生产部署。rootless、OpenSSH 与 MCP 探针都不是生产能力。

## 当前待办

- 通过首轮技术验证选定一个 Agent 适配器和实测模型组合。
- 验证 rootless 隔离构建，以及按 OCI digest 部署 Compose。
- ~~用两个客户端验证 rmcp 的 stdio/HTTP 接入。~~ **M0 探针已覆盖**（`musehub-mcp-probe`）；生产 MCP 与幂等/ OAuth 仍待实现。
- 以同一组任务体验 Gitea Enterprise、OneDev、GitLab Duo；记录版本、授权限制和操作证据。
- 冻结初始仓库规模、部署主机条件、数据保留期与许可证。
- 完成 M0 后再承诺工期和性能指标。
