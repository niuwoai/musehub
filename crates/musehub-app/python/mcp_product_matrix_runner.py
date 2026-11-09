#!/usr/bin/env python3
"""Production MCP matrix: official Python `mcp` SDK against product `/mcp` (not the M0 probe)."""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from typing import Any

PYTHON_MCP_SDK_VERSION = "2.3.0"
CLIENT_NAME = "musehub-product-mcp-matrix-python"
PRODUCT_SCOPE = "m1_product_mcp"
MCP_CONTRACT_VERSION = 1
PYTHON_HTTP_CLIENT_MODE = "legacy"


async def _exercise(client: Any, org_id: str) -> dict[str, Any]:
    checks: list[str] = ["initialize", "list_tools"]
    tools = await client.list_tools()
    names = {t.name for t in tools.tools}
    caps = await client.call_tool("musehub_capabilities_get", {})
    env = caps.structured_content or {}
    if env.get("schema_version") != MCP_CONTRACT_VERSION:
        raise RuntimeError(f"bad schema_version {env.get('schema_version')}")
    data = env.get("data") or {}
    if data.get("mcp_scope") != PRODUCT_SCOPE:
        raise RuntimeError(f"bad mcp_scope {data.get('mcp_scope')}")
    if data.get("production_claim"):
        raise RuntimeError("production_claim must not be set on product server")
    for t in data.get("implemented_tools") or []:
        if t not in names:
            raise RuntimeError(f"advertised tool missing from list_tools: {t}")
    checks.append("capabilities_get")
    listed = await client.call_tool(
        "musehub_repository_list", {"org_id": org_id, "limit": 5}
    )
    if not (listed.structured_content or {}).get("data"):
        raise RuntimeError("repository_list missing data")
    checks.append("repository_list")
    try:
        await client.call_tool("musehub_no_such_tool", {})
        raise RuntimeError("unknown tool should fail")
    except Exception:
        checks.append("unknown_tool_rejected")
    return {
        "client_name": CLIENT_NAME,
        "python_mcp_sdk_version": PYTHON_MCP_SDK_VERSION,
        "product_scope": PRODUCT_SCOPE,
        "checks_passed": checks,
        "negotiated_protocol": str(client.protocol_version),
    }


async def _main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True)
    parser.add_argument("--bearer", required=True)
    parser.add_argument("--org-id", required=True)
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
            out = await _exercise(client, args.org_id)
            print(json.dumps(out))


if __name__ == "__main__":
    asyncio.run(asyncio.wait_for(_main(), timeout=40))
