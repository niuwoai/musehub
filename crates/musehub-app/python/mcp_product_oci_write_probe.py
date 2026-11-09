#!/usr/bin/env python3
"""Python MCP SDK write probe: pipeline_run + deployment_start (official mcp SDK).

Uses REST only to poll pipeline run state between MCP writes (no pipeline_get tool).
Includes negative deployment_start and negative pipeline_run cases.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
import time
import uuid
from typing import Any

PYTHON_HTTP_CLIENT_MODE = "auto"


async def _poll_pipeline(
    http_client: Any,
    base: str,
    bearer: str,
    org: str,
    repo: str,
    pipeline_id: str,
    timeout_s: float = 300.0,
) -> str:
    url = f"{base.rstrip('/')}/api/v1/orgs/{org}/repos/{repo}/pipeline-runs/{pipeline_id}"
    headers = {"Authorization": f"Bearer {bearer}"}
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        r = await http_client.get(url, headers=headers)
        r.raise_for_status()
        st = r.json().get("state")
        if st in ("succeeded", "failed"):
            return str(st)
        await asyncio.sleep(0.5)
    raise TimeoutError("pipeline run poll timeout")


async def _main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True, help="MCP streamable HTTP base e.g. http://127.0.0.1:PORT/mcp")
    parser.add_argument("--bearer", required=True)
    parser.add_argument("--api-base", required=True, help="REST API origin e.g. http://127.0.0.1:PORT")
    parser.add_argument("--org-slug", default="demo")
    parser.add_argument("--repo-slug", required=True)
    parser.add_argument("--org-id", required=True)
    parser.add_argument("--repository-id", required=True)
    parser.add_argument("--environment-id", required=True)
    parser.add_argument("--release-id", required=True, help="Already published release for deployment_start")
    parser.add_argument("--target-oid", default="", help="40-char git oid for pipeline_run")
    parser.add_argument(
        "--mode",
        choices=("full", "lite"),
        default="full",
        help="lite = negatives + deployment_start only; full includes pipeline_run chain",
    )
    args = parser.parse_args()

    import httpx2
    from mcp import Client
    from mcp.client.streamable_http import streamable_http_client

    async with httpx2.AsyncClient(
        headers={"Authorization": f"Bearer {args.bearer}"},
        timeout=httpx2.Timeout(30.0, read=300.0),
    ) as http_client:
        transport = streamable_http_client(args.url, http_client=http_client)
        async with Client(transport, mode=PYTHON_HTTP_CLIENT_MODE) as client:
            bad_dep = await client.call_tool(
                "musehub_deployment_start",
                {
                    "org_id": args.org_id,
                    "repository_id": args.repository_id,
                    "environment_id": args.environment_id,
                    "release_id": str(uuid.uuid4()),
                    "idempotency_key": "py-write-probe-bad-release",
                },
            )
            if not bad_dep.is_error:
                raise RuntimeError("expected deployment_start error for unknown release")

            checks = [
                "deployment_start_negative_unknown_release",
            ]

            if args.mode == "full":
                if len(args.target_oid) != 40:
                    raise RuntimeError("full mode requires --target-oid (40-char git oid)")
                bad_pipe = await client.call_tool(
                    "musehub_pipeline_run",
                    {
                        "org_id": args.org_id,
                        "repository_id": args.repository_id,
                        "workflow_path": "product-ci",
                        "target_oid": "not-a-git-oid",
                        "idempotency_key": "py-write-probe-bad-oid",
                    },
                )
                if not bad_pipe.is_error:
                    raise RuntimeError("expected pipeline_run error for invalid target_oid")
                checks.append("pipeline_run_negative_bad_oid")

                pipe_key = "py-write-probe-pipeline-v1"
                pipe = await client.call_tool(
                    "musehub_pipeline_run",
                    {
                        "org_id": args.org_id,
                        "repository_id": args.repository_id,
                        "workflow_path": "product-ci",
                        "target_oid": args.target_oid,
                        "idempotency_key": pipe_key,
                    },
                )
                if pipe.is_error:
                    raise RuntimeError(f"pipeline_run failed: {pipe}")
                pipe_data = (pipe.structured_content or {}).get("data") or {}
                pipeline_id = pipe_data.get("id")
                if not pipeline_id:
                    raise RuntimeError("pipeline_run missing id")
                state = await _poll_pipeline(
                    http_client,
                    args.api_base,
                    args.bearer,
                    args.org_slug,
                    args.repo_slug,
                    str(pipeline_id),
                )
                if state != "succeeded":
                    raise RuntimeError(f"pipeline_run did not succeed: {state}")
                checks.extend(["pipeline_run_positive", "pipeline_run_idempotency"])

                pipe_replay = await client.call_tool(
                    "musehub_pipeline_run",
                    {
                        "org_id": args.org_id,
                        "repository_id": args.repository_id,
                        "workflow_path": "product-ci",
                        "target_oid": args.target_oid,
                        "idempotency_key": pipe_key,
                    },
                )
                if pipe_replay.is_error:
                    raise RuntimeError("pipeline_run idempotency replay failed")
                replay_id = ((pipe_replay.structured_content or {}).get("data") or {}).get("id")
                if str(replay_id) != str(pipeline_id):
                    raise RuntimeError("pipeline_run idempotency id mismatch")

            dep_key = "py-write-probe-deploy-v1"
            dep = await client.call_tool(
                "musehub_deployment_start",
                {
                    "org_id": args.org_id,
                    "repository_id": args.repository_id,
                    "environment_id": args.environment_id,
                    "release_id": args.release_id,
                    "idempotency_key": dep_key,
                },
            )
            if dep.is_error:
                raise RuntimeError(f"deployment_start failed: {dep}")
            dep_data = (dep.structured_content or {}).get("data") or {}
            if dep_data.get("state") not in ("succeeded", "running"):
                raise RuntimeError(f"unexpected deployment state: {dep_data}")
            dep_replay = await client.call_tool(
                "musehub_deployment_start",
                {
                    "org_id": args.org_id,
                    "repository_id": args.repository_id,
                    "environment_id": args.environment_id,
                    "release_id": args.release_id,
                    "idempotency_key": dep_key,
                },
            )
            if dep_replay.is_error:
                raise RuntimeError("deployment_start idempotency replay failed")

            checks.extend(["deployment_start_positive", "deployment_start_idempotency"])
            out = {
                "checks_passed": checks,
                "negotiated_protocol": str(client.protocol_version),
                "mode": args.mode,
            }
            print(json.dumps(out))


if __name__ == "__main__":
    asyncio.run(asyncio.wait_for(_main(), timeout=600))
