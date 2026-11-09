# 本地验收跑批记录

> Branch: `cursor/local-product-integration-5f97` → PR [#16](https://github.com/niuwoai/musehub/pull/16)  
> 跑批起点 SHA: `6f358426bfe4c7ea44ae8cbb56b4a412ecc55b74`  
> **未** `git push` 触发 GitHub Actions；无 merge/tag/生产发布。  
> DB: `postgres://musehub:musehub@127.0.0.1:5432/musehub`

## 摘要（收口轮）

| 类别 | Exit | 备注 |
|------|------|------|
| fmt / clippy / portable | 0 | `@e15554f`：`cargo fmt --all --check` + clippy `-D warnings` + `cargo-test-portable.sh` |
| product acceptance | 0 | `acceptance=ok`（含 `pipeline_runner_pat_e2e` 修复后） |
| CI security 镜像 | 0 | `ci_security_integration_local=ok`（~30 suite，单线程 DB） |
| backup / stack / compose / OCI | 0 | 见下表 #6–#13 |
| runner 子进程 + 跨容器 | 0 | cross-container：API 容器需 `git` + `MUSEHUB_DATABASE_URL` |
| web unit/build | 0 | vitest 1；vite build ok |
| Playwright live | 0 | `run-web-e2e.sh` **4** passed；`run-config-grants-e2e.sh` **2** passed |
| Playwright OCI 浏览器 | `[~]` | 本轮 `web_e2e product_oci=0`（未起本地 registry）；OCI 由 #12 `verify-pipeline-oci-product-e2e.sh` 覆盖 |

## 逐项命令

| # | 命令 | Exit | 测试/备注 | SHA |
|---|------|------|-----------|-----|
| 1 | `cargo fmt --all --check` | 0 | 首跑 @ `6f35842`；收口 @ `e15554f` | `e15554f` |
| 2 | `cargo clippy --workspace --all-targets -D warnings` | 0 | 收口 @ `e15554f` | `e15554f` |
| 3 | `./scripts/cargo-test-portable.sh` | 0 | workspace 便携单测 | `6f35842` / 见提交 |
| 4 | `./scripts/verify-product-acceptance.sh` | 0 | 首跑 pat 失败后 `1b77a60` 修复；收口轮 `acceptance=ok` | `1b77a60`+ |
| 5 | `./scripts/run-ci-security-integration-local.sh` | 0 | 首跑 127（env 引号）；修复脚本后 `ci_security_integration_local=ok` | `1b77a60`+ |
| 6 | `./scripts/verify-backup-restore.sh` | 0 | `backup_restore_verify=ok` | `1b77a60` |
| 7 | `./scripts/verify-remote-runner-e2e.sh` | 0 | `remote_runner_subprocess_e2e=ok` | `1b77a60` |
| 7b | `./scripts/verify-remote-deployment-e2e.sh` | 0 | `remote_deployment_subprocess_e2e=ok`；API `202` + worker claim | 本 commit |
| 7c | `cargo test -p musehub-app --test remote_deployment_runner_e2e` | 0 | in-process claim + `execute_deployment_job` | 本 commit |
| 7d | `./scripts/verify-product-production-stack-e2e.sh` | `[~]` | 拓扑（无 API docker.sock）+ capabilities；全链 agent/OCI/deploy 见工单 follow-up | 本 commit |
| 8 | `./scripts/verify-runner-cancel-subprocess-e2e.sh` | 0 | cancel + 无 ready artifact | `1b77a60` |
| 9 | `./scripts/verify-runner-kill-reclaim-e2e.sh` | 0 | SIGKILL + `runner_reclaim_build_e2e` | `1b77a60` |
| 10 | `./scripts/verify-runner-cross-container-e2e.sh` | 0 | `runner_cross_container_e2e=ok`；**非**真实双主机 | `e15554f` |
| 11 | `./scripts/verify-compose-runner-e2e.sh` | 0 | digest 校验 compose | `1b77a60` |
| 12 | `./scripts/verify-pipeline-oci-product-e2e.sh` | 0 | Python full 写链 + 双 digest + HTTP/mock 回滚 | `1b77a60` |
| 13 | `./scripts/verify-product-stack-e2e.sh` | 0 | `product_stack_e2e=ok` | `1b77a60` |
| 14 | `yarn --cwd web test` | 0 | 1 test | 收口轮 |
| 15 | `yarn --cwd web build` | 0 | vite production | 收口轮 |
| 16 | `./scripts/run-web-e2e.sh` | 0 | Playwright **4**（product-flow, config-grants×2, delivery mock） | 收口轮 |
| 17 | `./scripts/run-config-grants-e2e.sh` | 0 | Playwright **2**；`config_grants_playwright=ok` | 收口轮 |

## 失败与修复（本轮）

| 项 | 原因 | 修复 |
|----|------|------|
| `pipeline_runner_pat_e2e` | 默认 PAT `scope_mode=selected` 缺 `repo_ids` | runner token 改 `all_accessible` → `1b77a60` |
| `run-ci-security-integration-local.sh` | `env KEY=val _one` 拆词失败 exit 127 | 引号修复 + 脚本入库 |
| cross-container exit 22 | API 容器无 `git`，建库 500 | 容器 `apt install git`；runner `MUSEHUB_DATABASE_URL` |
| cross-container 与 migrate 并行 | duplicate pg_type | **勿与** #5 并行跑 migrate |

## 仅外部 / 未本地替代

- **真实双主机** runner（README 独立 Linux Runner 更广目标）：以跨容器 + 共享 DB/卷代替，见 `docs/ACCEPTANCE-EVIDENCE.md`。
- **Live Codex**：需 `MUSEHUB_AGENT_CODEX_BIN` + 凭据；镜像仅 `codex-sim`（`deploy/RUNBOOK.md` § What is not）。
- **GitHub Actions**：与用户统一触发，本次未 push 跑 CI。
