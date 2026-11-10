# Runner 与进程边界（产品诚实声明）

MuseHub 首版在 **同一 API 进程** 内调度以下工作负载，**不等价**于独立、远程、租户隔离的 Runner 服务。

| 组件 | 实际形态 | 隔离证据 |
|------|----------|----------|
| Agent 执行 | `MUSEHUB_AGENT_EXECUTOR=remote` 时 API 仅入队；`musehub-runnerd` claim + 本地适配器（reference / codex-sim / `MUSEHUB_AGENT_CODEX_BIN` live） | `remote_agent_runner_e2e`、`verify-remote-agent-e2e.sh`；demo 镜像默认 in-process |
| Deploy 执行 | `MUSEHUB_DEPLOY_EXECUTOR=remote` 时 API 写入 `queued` + `launch_payload`；worker `claim` + `execute_deployment_job`（compose-mock/runner）；API 容器无 `docker.sock` | `remote_deployment_runner_e2e`、`verify-remote-deployment-e2e.sh`；production profile 强制三者皆 remote |
| Pipeline | 进程内 `PipelineSupervisor` + `musehub-pipeline-*` 子进程 | `pipeline_*_e2e` |
| 部署 | `compose-mock` / `compose-runner` **子进程**（可配置二进制路径） | `compose-runner-e2e`（需 Docker）；产品栈镜像默认 **mock** |
| `musehub-runner-probe` | 本地进程超时/取消探针 | `SCOPE=local_process_only`；**非** rootless/OCI 证明 |

## 产品 Docker 栈（`deploy/`）

- **不提供** `docker.sock` 挂载。
- **不** 将任意 HTTP 请求映射为 shell 执行。
- Compose 适配器为 **编译进镜像的固定二进制**（默认 `compose-mock`）。

## 远程 Pipeline Runner（V1 局部交付）

| 能力 | V1 状态 |
|------|---------|
| 控制面不挂载 `docker.sock` | `[x]` 产品镜像/compose 均无 socket 卷 |
| `MUSEHUB_PIPELINE_EXECUTOR=remote` | `[x]` API 仅入队，不在进程内 `tokio::spawn` pipeline |
| `musehub-runnerd` 常驻 worker | `[x]` PAT + `POST /api/v1/runner/pipeline-runs/claim`；lease renew；`execute_pipeline_job` 本地适配器 |
| Worker 数据库 | `[~]` runner **需** `MUSEHUB_DATABASE_URL` 写 artifact 元数据（无纯 HTTP 上传链） |
| Agent 远端 worker | `[x]` PAT `agent:execute` + `POST /api/v1/runner/agent-runs/claim`；共享 DB/git 卷 |
| Compose 远端化 | `[~]` 子进程 `compose-runner`；生产 profile 要求 `runner` 模式 |

诚实边界：runner 与 API 可分机部署，但 V1 仍共享 DB 与（通常）共享 `MUSEHUB_GIT_ROOT`/`MUSEHUB_ARTIFACT_ROOT` 挂载；非完全零信任 multi-tenant worker。

恢复/重试语义见 [RUNNER-RECOVERY.md](RUNNER-RECOVERY.md)（租约 TTL 重认领、epoch 围栏、reconcile 与重试边界）。

### 子进程验收（VM 本地，非 GitHub Actions 唯一完成标准）

| 脚本 | 作用 |
|------|------|
| `scripts/verify-runner-cancel-subprocess-e2e.sh` | 慢 adapter + HTTP cancel；进程组退出；无 ready release/deploy |
| `scripts/verify-runner-kill-reclaim-e2e.sh` | 真实 `kill -9 musehub-runnerd`；租约 TTL；第二 worker `lease_holder` + `runner_reclaim_build_e2e` |
| `scripts/verify-runner-cross-container-e2e.sh` | 两容器 + 共享 DB/git/artifact 拓扑（诚实：非双主机） |

远程 worker 执行 adapter 时轮询 `cancel_requested`（`wait_pipeline_child`），不再依赖控制面本地的 `PipelineSupervisor` PID。

## MCP / 写路径诚实性

- **Rust rmcp** 在 `pipeline-oci-product` job 内验收 `pipeline_run` / `deployment_start` 写路径。
- **Python** `mcp_product_oci_read_probe.py`：`deployment_trace` + `capabilities_get`（只读）。
- **Python** `mcp_product_oci_write_probe.py`：官方 MCP SDK 写链（`lite` = 负例 + `deployment_start` 幂等；`full` = 另加 `pipeline_run` 链 + REST 轮询状态），在 `pipeline_oci_product_e2e` 跑 **lite**；**full** 由 `verify-pipeline-oci-product-e2e.sh` 在同一 Docker job 内附加调用（需已发布 release 与 PAT 参数）。

## 部署回滚（产品语义）

- `POST .../deployments/rollback` **必须** 经与 `deployment` 相同的 compose 子进程（mock/runner），重新部署目标行的 **published release**（含 digest/OCI 校验）。
- **禁止** 仅拷贝 `health_summary` 伪造 `succeeded`。
- Runner 回滚路径：可选拆除上一 `compose_project`（`MUSEHUB_COMPOSE_TEARDOWN_PROJECT`），环境级项目名 `musehub-env-{environment_id}`，`LEAVE_STACK=1` 保持运行栈。
