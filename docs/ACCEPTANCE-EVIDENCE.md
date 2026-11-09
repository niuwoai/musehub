# P0 / README 验收 ↔ 证据对照（诚实）

> 仅列本仓库可重复跑到的证据；**双主机物理分列**仅在 README 原文明确要求时分项（当前以**跨容器 + 共享 DB/卷**代替，见 `scripts/verify-runner-cross-container-e2e.sh`）。

| ID / 主题 | README / 01 | 状态 | 证据 |
|-----------|-------------|------|------|
| CI-02 远程 Pipeline / 过期 worker 不能写成功 | `01-product-requirements.md` CI-02 | `[~]` pipeline 切片 | `musehub-runnerd` + `runner_recovery_e2e` + `runner_reclaim_build_e2e` + `verify-runner-kill-reclaim-e2e.sh` + `verify-runner-cross-container-e2e.sh`（2026-11-09 本地 exit 0） |
| AGT-02 取消停止执行 | AGT-02 | `[~]` pipeline 切片 | 远程 worker `cancel_flag` 轮询 + `pipeline_supervisor` 杀进程组；`verify-runner-cancel-subprocess-e2e.sh` |
| 控制面无 docker.sock | 架构 / DEP | `[x]` | `deploy/` 无 socket；`RUNNER-BOUNDARY.md` |
| Agent 远端 worker | README 独立 Linux Runner | `[~]` | `musehub-runnerd` + claim/renew/fence/cancel；`verify-remote-agent-e2e.sh`；共享 DB/卷；非双主机 |
| Compose 远端 worker | DEP / compose-runner | `[~]` | **子进程** `compose-runner`（可配路径），非常驻 worker；`verify-compose-runner-e2e.sh` |
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
| `run-ci-security-integration-local.sh` | 本地镜像 CI security/MCP/runner 矩阵（单线程 DB） |
| `docs/LOCAL-ACCEPTANCE-RUN.md` | 本地跑批命令/exit/SHA 台账 |
