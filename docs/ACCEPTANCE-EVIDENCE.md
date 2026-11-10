# P0 / README 验收 ↔ 证据对照（诚实）

> 仅列本仓库可重复跑到的证据。

### 独立 Linux Runner vs 物理双主机（对照 `docs/01-product-requirements.md` / `README.md`）

| 原文要求 | V1 范围结论 | 本仓库本地证据 |
|---|---|---|
| `README.md` 设计基线：「模块化单体控制面 + **独立 Linux Runner**」 | **在 V1 内**（控制面与 Runner 分进程/分容器，非 Windows/macOS Runner） | `musehub-runnerd`、production compose、`verify-remote-*-e2e.sh` |
| `docs/01-product-requirements.md` §3 **CI-02** Linux Runner | **在 V1 内** | 同上 + `run-ci-security-integration-local.sh` remote executor 步 |
| `docs/01-product-requirements.md` §3 **明确排除**（约 L79）：Windows/macOS Runner、**多地域 HA** | **未**将「第二台物理 Linux 机」列为 V1 外 | — |
| 实验室「两台物理机」拓扑证据 | 非产品契约；仅验收诚实性 | `scripts/verify-runner-cross-container-e2e.sh`（**跨容器 + host-network + 共享 DB/卷**，[`docs/RUNNER-BOUNDARY.md`](RUNNER-BOUNDARY.md) 已标明非双主机） |

| ID / 主题 | README / 01 | 状态 | 证据 |
|-----------|-------------|------|------|
| CI-02 远程 Pipeline / 过期 worker 不能写成功 | `01-product-requirements.md` CI-02 | `[x]` 本地 | `remote_pipeline_runner_e2e` + `verify-remote-runner-e2e.sh` + `verify-runner-kill-reclaim-e2e.sh` + `run-ci-security-integration-local.sh` |
| AGT-02 取消停止执行 | AGT-02 | `[x]` 本地 | `verify-remote-agent-cancel-e2e.sh` + `verify-remote-agent-kill-reclaim-e2e.sh` + `remote_agent_epoch_e2e` + `agent_cancel_e2e` |
| DEP 远端 worker deploy | DEP-01 / README 主线 | `[x]` 本地 | `verify-remote-deployment-e2e.sh` + `remote_deployment_runner_e2e` + `verify-product-production-stack-e2e.sh` |
| 控制面无 docker.sock | 架构 / DEP | `[x]` | production stack 断言；`RUNNER-BOUNDARY.md` |
| Agent 远端 worker | README 独立 Linux Runner | `[x]` 本地 | `verify-remote-agent-e2e.sh` + production stack agent 步；**非**物理双主机 |
| Compose 远端 worker | DEP / compose-runner | `[x]` 本地 | `verify-compose-runner-e2e.sh` + production stack OCI deploy |
| 干净安装 / Runbook | OPS-01 | `[x]` 本地 | `verify-clean-install-runbook.sh` + `install-production-compose.sh` + `deploy/RUNBOOK.md` |
| `selected` PAT 选仓 | 最小 RBAC | `[~]` | `grants_security_e2e`, `production_mcp_selected_repo_e2e`, Playwright `run-config-grants-e2e.sh` |
| 用户可编辑配置 | OPS | `[~]` | `organizations.preferences` + Web Playwright |
| OAuth / 多区域 HA | V1 外 | `[ ]` | 未扩展 |

### 子进程 Runner 脚本

| 脚本 | 证明内容 |
|------|----------|
| `verify-remote-runner-e2e.sh` | `MUSEHUB_PIPELINE_EXECUTOR=remote` + 共享 DB/卷 |
| `verify-runner-cancel-subprocess-e2e.sh` | server + runnerd + `musehub-pipeline-slow`；HTTP cancel；adapter 退出；无 ready artifact |
| `verify-runner-kill-reclaim-e2e.sh` | `kill -9 runnerd`；租约 TTL；第二 worker `lease_holder`；`runner_reclaim_build_e2e` 唯一 ready artifact |
| `verify-runner-cross-container-e2e.sh` | 两 Docker 容器 + `--network host` + 共享 DB/卷（**非真实双主机**）；API/runner 容器内 `git` |
| `run-ci-security-integration-local.sh` | 本地镜像 CI security/MCP/runner + **remote deploy/agent cancel/kill/budget** 子进程脚本 |
| `verify-product-production-stack-e2e.sh` | 生产 compose 全链（无 API docker.sock） |
| `verify-clean-install-runbook.sh` | RUNBOOK quick start 实测 |
| `docs/LOCAL-ACCEPTANCE-RUN.md` | 本地跑批命令/exit/SHA 台账 |
