#!/usr/bin/env bash
# Hermetic acceptance gate for the docs_search MCP tool.
#
# Builds a FROM-scratch image containing ONLY the compiled `sema` binary, then drives
# `sema mcp` over stdio JSON-RPC under `--network none` and asserts that docs_search
# returns real, relevant results. This is "un-fudgeable": the container has no repo
# source and no uncompiled docs, so a pass proves the corpus + index are baked into the
# binary and need no LLM/network at query time.
#
# Requires: docker, jq. Usage: scripts/docs-search-gate.sh
set -euo pipefail

IMG="sema-docs-search-gate:test"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

command -v docker >/dev/null || { echo "FAIL: docker not found"; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq not found"; exit 1; }

echo "==> Building hermetic image (binary-only, FROM scratch)"
docker build -f "$ROOT/Dockerfile.docs-search-gate" -t "$IMG" "$ROOT"

# Drive a full MCP session: initialize, list tools, then call docs_search. Closing
# stdin (the heredoc EOF) makes the server exit cleanly.
echo "==> Running docs_search inside the container (--network none)"
REQS=$(cat <<'JSON'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"gate","version":"1.0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"docs_search","arguments":{"query":"apply a function to every element of a list"}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"docs_search","arguments":{"query":"decode a json string into a value"}}}
JSON
)

OUT=$(printf '%s\n' "$REQS" | docker run -i --rm --network none "$IMG" mcp 2>/dev/null)

fail() { echo "FAIL: $1"; echo "--- raw output ---"; echo "$OUT"; exit 1; }

# Each response is one JSON object per line; correlate by id (never by position).
list=$(echo "$OUT" | jq -c 'select(.id==2)') || fail "no tools/list response"
echo "$list" | jq -e '.result.tools | map(.name) | index("docs_search")' >/dev/null \
  || fail "tools/list does not advertise docs_search"

r3=$(echo "$OUT" | jq -c 'select(.id==3)') || fail "no docs_search response (id 3)"
echo "$r3" | jq -e '.result.isError == false' >/dev/null || fail "docs_search reported isError"
echo "$r3" | jq -e '.result.content[0].type == "text"' >/dev/null || fail "result is not a text block"
text=$(echo "$r3" | jq -r '.result.content[0].text')
[ -n "${text// /}" ] || fail "result text is empty"
# The text is itself a JSON array of hits; assert `map` surfaced (relevance), proving
# this is real ranking and not an echo of the query.
echo "$text" | jq -e 'map(.name) | index("map")' >/dev/null \
  || { echo "got: $text"; fail "expected 'map' among results for a list-transform query"; }

r4=$(echo "$OUT" | jq -c 'select(.id==4)') || fail "no docs_search response (id 4)"
t4=$(echo "$r4" | jq -r '.result.content[0].text')
echo "$t4" | jq -e 'map(.name) | any(startswith("json/"))' >/dev/null \
  || { echo "got: $t4"; fail "expected a json/* entry for a json-decode query"; }

echo "PASS: docs_search returned relevant results from the binary alone (no source, no network)"
