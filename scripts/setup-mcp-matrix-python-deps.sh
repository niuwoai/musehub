#!/usr/bin/env bash
# Install locked Python MCP SDK into target/mcp-matrix-pydeps for matrix tests.
# Run once before `cargo test -p musehub-mcp-probe` when you want zero pip during tests.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEPS="$ROOT/target/mcp-matrix-pydeps"
REQ="$ROOT/crates/musehub-mcp-probe/python/requirements.txt"
MARKER="$DEPS/.mcp-2.3.0-installed"
mkdir -p "$DEPS"
pip3 install --quiet --disable-pip-version-check --target "$DEPS" -r "$REQ"
echo ok >"$MARKER"
python3 -c "import sys; sys.path.insert(0, '$DEPS'); import mcp; print('mcp ready')"
