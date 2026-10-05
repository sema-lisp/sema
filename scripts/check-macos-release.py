#!/usr/bin/env python3
"""Check a distributed macOS executable without changing its signature."""

import argparse
import json
from pathlib import Path
import re
import subprocess


def non_system_libraries(listing):
    libraries = re.findall(r"^\s+(.+?) \(compatibility version", listing, re.MULTILINE)
    if not libraries:
        raise ValueError("otool did not report any linked libraries")
    return [p for p in libraries if not p.startswith(("/usr/lib/", "/System/Library/"))]


def check(binary, version):
    binary = str(Path(binary).resolve())
    listing = subprocess.check_output(["otool", "-arch", "all", "-L", binary], text=True)
    unexpected = non_system_libraries(listing)
    if unexpected:
        raise RuntimeError(f"non-system dynamic libraries: {unexpected}")
    subprocess.run(["codesign", "--verify", "--strict", "--all-architectures", binary], check=True)
    info = subprocess.check_output(
        ["codesign", "--display", "--verbose=4", binary], stderr=subprocess.STDOUT, text=True
    )
    if "runtime" not in info or "Authority=Developer ID Application:" not in info:
        raise RuntimeError("expected a Developer ID signature with hardened runtime")
    actual = subprocess.check_output([binary, "--version"], text=True, timeout=30).strip()
    if actual != f"sema {version}":
        raise RuntimeError(f"unexpected version: {actual!r}")
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "release-smoke", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    ]
    result = subprocess.run(
        [binary, "mcp"],
        input="".join(json.dumps(m) + "\n" for m in messages),
        capture_output=True, text=True, timeout=30, check=True,
    )
    replies = {m["id"]: m for line in result.stdout.splitlines()
               if "id" in (m := json.loads(line))}
    if replies.get(1, {}).get("result", {}).get("serverInfo", {}).get("version") != version:
        raise RuntimeError(f"MCP initialization failed: {result.stdout}")
    names = {t["name"] for t in replies.get(2, {}).get("result", {}).get("tools", [])}
    if not {"eval", "run_file"}.issubset(names):
        raise RuntimeError(f"MCP tools missing: {names}")
    print(f"macOS release verified: {binary} ({version})")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("version")
    args = parser.parse_args()
    check(args.binary, args.version)
