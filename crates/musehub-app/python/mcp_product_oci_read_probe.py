#!/usr/bin/env python3
"""Python MCP SDK read-only probe on product /mcp (deployment_trace + capabilities_get).

Does not call pipeline_run, deployment_start, or other write tools. Real Python write
acceptance remains a separate follow-up job.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from typing import Any

PYTHON_HTTP_CLIENT_MODE = "auto"


async def _main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True)
    parser.add_argument("--bearer", required=True)
    parser.add_argument("--org-id", required=True)
    parser.add_argument("--repository-id", required=True)
    parser.add_argument("--deployment-id", required=True)
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
            trace = await client.call_tool(
                "musehub_deployment_trace",
                {
                    "org_id": args.org_id,
                    "repository_id": args.repository_id,
                    "deployment_id": args.deployment_id,
                },
            )
            env = trace.structured_content or {}
            if env.get("schema_version") != 1:
                raise RuntimeError("bad schema_version")
            data = (env.get("data") or {}).get("gaps")
            if data is None:
                raise RuntimeError("deployment_trace missing gaps")
            caps = await client.call_tool("musehub_capabilities_get", {})
            cap_data = (caps.structured_content or {}).get("data") or {}
            templates = cap_data.get("resource_uri_templates_v1") or []
            if len(templates) < 5:
                raise RuntimeError("resource_uri_templates_v1 incomplete")
            reads = cap_data.get("resource_reads_implemented_v1") or []
            if not reads:
                raise RuntimeError("resource_reads_implemented_v1 missing")
            out: dict[str, Any] = {
                "checks_passed": [
                    "deployment_trace_read",
                    "capabilities_get_read",
                    "resource_reads_implemented_v1",
                ],
                "negotiated_protocol": str(client.protocol_version),
            }
            print(json.dumps(out))


if __name__ == "__main__":
    asyncio.run(asyncio.wait_for(_main(), timeout=120))
