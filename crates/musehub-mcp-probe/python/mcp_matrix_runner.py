#!/usr/bin/env python3
"""M0 MCP client matrix runner using the official Python `mcp` SDK (not rmcp).

Emits a single JSON object to stdout for Rust integration tests to parse.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from typing import Any

PYTHON_MCP_SDK_VERSION = "2.3.0"
CLIENT_IMPLEMENTATION = "python-mcp-sdk"
CLIENT_NAME = "musehub-mcp-matrix-python"
PROBE_SCOPE = "m0_mcp_probe"
MCP_CONTRACT_VERSION = 1
SAMPLE_ERROR_CODE = "PROBE_SAMPLE_ERROR"
EXPECTED_TOOLS = (
    "musehub_capabilities_get",
    "musehub_probe_echo",
    "musehub_probe_error_sample",
)


async def _exercise(client: Any, *, bearer_gate: bool) -> dict[str, Any]:
    checks: list[str] = ["initialize", "list_tools"]
    negotiated = str(client.protocol_version)

    tools_result = await client.list_tools()
    names = sorted(tool.name for tool in tools_result.tools)
    for expected in EXPECTED_TOOLS:
        if expected not in names:
            raise RuntimeError(f"missing tool {expected} in {names}")

    caps = await client.call_tool("musehub_capabilities_get", {})
    envelope = caps.structured_content
    if envelope is None:
        raise RuntimeError("capabilities missing structured_content")
    if envelope.get("schema_version") != MCP_CONTRACT_VERSION:
        raise RuntimeError(f"unexpected schema_version {envelope.get('schema_version')}")
    data = envelope.get("data") or {}
    if data.get("probe_scope") != PROBE_SCOPE:
        raise RuntimeError(f"unexpected probe_scope {data.get('probe_scope')}")
    checks.append("capabilities_get")

    echo = await client.call_tool("musehub_probe_echo", {"message": "matrix-ping"})
    echo_env = echo.structured_content
    if not echo_env or (echo_env.get("data") or {}).get("echo") != "matrix-ping":
        raise RuntimeError(f"unexpected echo payload {echo_env}")
    checks.append("probe_echo")

    try:
        await client.call_tool("musehub_probe_echo", {"message": "   "})
        raise RuntimeError("empty echo should have been rejected")
    except Exception:
        checks.append("invalid_echo_rejected")

    sample = await client.call_tool("musehub_probe_error_sample", {})
    if not getattr(sample, "is_error", False):
        raise RuntimeError("error sample must set is_error")
    sample_env = sample.structured_content
    error = (sample_env or {}).get("error") or {}
    if error.get("code") != SAMPLE_ERROR_CODE:
        raise RuntimeError(f"unexpected error code {error.get('code')}")
    checks.append("error_sample_envelope")

    try:
        await client.call_tool("musehub_no_such_tool", {})
        raise RuntimeError("unknown tool should error")
    except Exception:
        checks.append("unknown_tool_rejected")

    if bearer_gate:
        checks.append("bearer_auth_gate")

    return {
        "negotiated_protocol": negotiated,
        "tools_discovered": names,
        "checks": checks,
    }


async def run_stdio(stdio_bin: str) -> dict[str, Any]:
    from mcp import Client, StdioServerParameters

    server = StdioServerParameters(command=stdio_bin, args=[])
    async with Client(server, mode="2026-07-28") as client:
        body = await _exercise(client, bearer_gate=False)
    return {
        "client_implementation": CLIENT_IMPLEMENTATION,
        "client_sdk_version": PYTHON_MCP_SDK_VERSION,
        "client_name": CLIENT_NAME,
        "transport": "stdio",
        "passed": True,
        **body,
    }


async def run_http(url: str, bearer: str) -> dict[str, Any]:
    import httpx2
    from mcp import Client
    from mcp.client.streamable_http import streamable_http_client

    async with httpx2.AsyncClient(
        headers={"Authorization": f"Bearer {bearer}"},
        timeout=httpx2.Timeout(30.0, read=300.0),
    ) as http_client:
        transport = streamable_http_client(url, http_client=http_client)
        # rmcp probe server uses classic initialize over Streamable HTTP sessions.
        async with Client(transport, mode="legacy") as client:
            body = await _exercise(client, bearer_gate=True)
    return {
        "client_implementation": CLIENT_IMPLEMENTATION,
        "client_sdk_version": PYTHON_MCP_SDK_VERSION,
        "client_name": CLIENT_NAME,
        "transport": "streamable_http",
        "passed": True,
        **body,
    }


async def verify_http_bearer_rejects(url: str) -> dict[str, Any]:
    import httpx2
    from mcp import Client
    from mcp.client.streamable_http import streamable_http_client

    async with httpx2.AsyncClient(timeout=httpx2.Timeout(10.0)) as http_client:
        transport = streamable_http_client(url, http_client=http_client)
        try:
            async with Client(transport, mode="2026-07-28") as _client:
                raise RuntimeError("missing bearer must not initialize")
        except Exception:
            return {"transport": "streamable_http", "check": "missing_bearer_rejected", "passed": True}

    return {"transport": "streamable_http", "check": "missing_bearer_rejected", "passed": False}


async def main_async() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--mode",
        choices=["stdio", "http", "http-bearer-reject"],
        required=True,
    )
    parser.add_argument("--stdio-bin", default="")
    parser.add_argument("--http-url", default="")
    parser.add_argument("--bearer", default="")
    args = parser.parse_args()

    if args.mode == "stdio":
        if not args.stdio_bin:
            print("stdio-bin required", file=sys.stderr)
            return 2
        report = await run_stdio(args.stdio_bin)
    elif args.mode == "http":
        if not args.http_url or not args.bearer:
            print("http-url and bearer required", file=sys.stderr)
            return 2
        report = await run_http(args.http_url, args.bearer)
    else:
        if not args.http_url:
            print("http-url required", file=sys.stderr)
            return 2
        report = await verify_http_bearer_rejects(args.http_url)

    json.dump(report, sys.stdout)
    sys.stdout.write("\n")
    return 0


def main() -> None:
    raise SystemExit(asyncio.run(main_async()))


if __name__ == "__main__":
    main()
