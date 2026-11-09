# Runner 与进程边界（产品诚实声明）

MuseHub 首版在 **同一 API 进程** 内调度以下工作负载，**不等价**于独立、远程、租户隔离的 Runner 服务。

| 组件 | 实际形态 | 隔离证据 |
|------|----------|----------|
| Agent 执行 | `musehub-server` 内 `AgentSupervisor` + 子进程适配器 | `agent_*_e2e`；**非** 远端 worker 注册 |
| Pipeline | 进程内 `PipelineSupervisor` + `musehub-pipeline-*` 子进程 | `pipeline_*_e2e` |
| 部署 | `compose-mock` / `compose-runner` **子进程**（可配置二进制路径） | `compose-runner-e2e`（需 Docker）；产品栈镜像默认 **mock** |
| `musehub-runner-probe` | 本地进程超时/取消探针 | `SCOPE=local_process_only`；**非** rootless/OCI 证明 |

## 产品 Docker 栈（`deploy/`）

- **不提供** `docker.sock` 挂载。
- **不** 将任意 HTTP 请求映射为 shell 执行。
- Compose 适配器为 **编译进镜像的固定二进制**（默认 `compose-mock`）。

## 远程 Runner（未交付）

P0 缺口：Runner 注册、心跳、租约、跨节点网络策略与不可信 job 隔离仍在 `docs/PROGRESS.md` 标注为未产品化。

## MCP / 写路径诚实性

- **Rust rmcp** 在 `pipeline-oci-product` job 内验收 `pipeline_run` / `deployment_start` 写路径。
- **Python** `mcp_product_oci_read_probe.py`：`deployment_trace` + `capabilities_get`（只读）。
- **Python** `mcp_product_oci_write_probe.py`：官方 MCP SDK 写链（`lite` = 负例 + `deployment_start` 幂等；`full` = 另加 `pipeline_run` 链 + REST 轮询状态），在 `pipeline_oci_product_e2e` 跑 **lite**；**full** 由 `verify-pipeline-oci-product-e2e.sh` 在同一 Docker job 内附加调用（需已发布 release 与 PAT 参数）。

## 部署回滚（产品语义）

- `POST .../deployments/rollback` **必须** 经与 `deployment` 相同的 compose 子进程（mock/runner），重新部署目标行的 **published release**（含 digest/OCI 校验）。
- **禁止** 仅拷贝 `health_summary` 伪造 `succeeded`。
- Runner 回滚路径：可选拆除上一 `compose_project`（`MUSEHUB_COMPOSE_TEARDOWN_PROJECT`），环境级项目名 `musehub-env-{environment_id}`，`LEAVE_STACK=1` 保持运行栈。
