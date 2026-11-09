# 本地验收跑批记录

> Branch: `cursor/local-product-integration-5f97` → PR [#16](https://github.com/niuwoai/musehub/pull/16)  
> 跑批起点 SHA: 见本 commit `git rev-parse HEAD`  
> **未** `git push` 触发 GitHub Actions；无 merge/tag/生产发布。  
> DB: `postgres://musehub:musehub@127.0.0.1:5432/musehub`

## 摘要（production stack 收口）

| 类别 | Exit | 备注 |
|------|------|------|
| `verify-product-production-stack-e2e.sh` | 0 | `product_production_stack_e2e=ok`（bootstrap→PAT→worker→agent/PR/CI/merge→OCI→deploy/rollback/trace）；API 无 docker.sock |
| `run-ci-security-integration-local.sh` | 0 | 含 `remote_deployment_runner_e2e`、`remote_agent_epoch_e2e` + 远端 agent 子进程脚本 |
| `verify-pipeline-oci-product-e2e.sh` | 0 | `sg docker`；`pipeline_oci_product=ok` |
| `verify-product-stack-e2e.sh` | 0 | `product_stack_e2e=ok` |
| `verify-product-production-profile.sh` | 0 | `product_production_profile_fail_closed=ok` |
| `run-web-e2e.sh` `MUSEHUB_PRODUCT_OCI_E2E=1` | 0 | Playwright **4 passed**；remote `pipeline/agent/deploy` worker |

## 关键修复（本 commit）

| 项 | 原因 | 修复 |
|----|------|------|
| `curl: (2) no URL` | PAT `curl` 缺 URL | `verify-product-production-stack-e2e.sh` |
| Pipeline OCI on worker | docker CLI API 过旧 / 无 compose 插件 | Dockerfile 静态 docker + compose 插件；CI overlay 挂 host CLI/插件 |
| Deploy GET 404 | 无 REST `GET .../deployments/{id}` | `deployment::get_deployment` + 路由 |
| Deploy `running` 卡死 | worker 默认 `compose-mock` + OCI | `MUSEHUB_COMPOSE_ADAPTER_MODE=runner` on worker |
| `product_oci=1` Playwright queued | 仅 pipeline remote worker | `run-web-e2e.sh`：`all` work kinds + remote agent/deploy |

## 逐项命令（节选）

| # | 命令 | Exit | 备注 |
|---|------|------|------|
| P1 | `./scripts/verify-product-production-stack-e2e.sh` | 0 | log: `/tmp/prod-full14.log` |
| P2 | `./scripts/run-ci-security-integration-local.sh` | 0 | `/tmp/accept-security.log` |
| P3 | `sg docker -c ./scripts/verify-pipeline-oci-product-e2e.sh` | 0 | 勿与 migrate 并行 |
| P4 | `./scripts/verify-product-stack-e2e.sh` | 0 | |
| P5 | `./scripts/verify-product-production-profile.sh` | 0 | |
| P6 | `sg docker -c 'MUSEHUB_PRODUCT_OCI_E2E=1 ./scripts/run-web-e2e.sh'` | 0 | |

## 仅外部 / 未本地替代

- **真实双主机** runner：以 compose + host-network worker 代替（见 `RUNNER-BOUNDARY.md`）。
- **Live Codex**：需 `MUSEHUB_AGENT_CODEX_BIN` + 凭据。
- **GitHub Actions**：用户统一触发。
