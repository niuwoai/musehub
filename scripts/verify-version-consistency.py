#!/usr/bin/env python3
"""Fail when application version metadata drifts across workspace surfaces."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
EXPECTED = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
errors: list[str] = []


def require_version(label: str, actual: str) -> None:
    if actual != EXPECTED:
        errors.append(f"{label}: expected {EXPECTED}, found {actual}")


for manifest_path in sorted((ROOT / "crates").glob("*/Cargo.toml")):
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    package = manifest.get("package", {})
    if package.get("name", "").startswith("musehub-"):
        require_version(str(manifest_path.relative_to(ROOT)), package.get("version", "<missing>"))

lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
for package in lock.get("package", []):
    if package.get("name", "").startswith("musehub-"):
        require_version(f"Cargo.lock:{package['name']}", package.get("version", "<missing>"))

web = json.loads((ROOT / "web/package.json").read_text(encoding="utf-8"))
require_version("web/package.json", web.get("version", "<missing>"))

for relative in ("README.md", "docs/02-architecture.md", "docs/product-overview.md"):
    text = (ROOT / relative).read_text(encoding="utf-8")
    versions = set(re.findall(r"\b\d+\.\d+\.\d+-rc\d+\b", text))
    if versions != {EXPECTED}:
        errors.append(f"{relative}: expected only {EXPECTED}, found {', '.join(sorted(versions)) or '<none>'}")

changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
first_heading = next((line for line in changelog.splitlines() if line.startswith("## [")), "")
if first_heading != f"## [{EXPECTED}]":
    errors.append(f"CHANGELOG.md: first release heading should be ## [{EXPECTED}], found {first_heading or '<missing>'}")

if errors:
    print("version_consistency=fail", file=sys.stderr)
    for error in errors:
        print(f"- {error}", file=sys.stderr)
    sys.exit(1)

print(f"version_consistency=ok version={EXPECTED}")
