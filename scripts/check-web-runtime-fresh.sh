#!/usr/bin/env bash

# Is the committed browser runtime (crates/sema/src/web/assets/) a current build
# of its sources?
#
# This replaces a rebuild-and-byte-compare check, which could never pass. rustc
# derives each crate's disambiguator (`wasm_bindgen_<16 hex>`, which appears in
# both the wasm and the JS glue) from host-dependent inputs, so a macOS laptop
# and a Linux runner produce different bytes from identical source — verified:
# after remapping every absolute path out of the build, 234 bytes and 16 glue
# lines still differed, and changing the remap flags did not move the hash. No
# contributor could satisfy that gate from their own machine, so it sat red for
# 16 days and hid two real bugs behind it.
#
# Comparing only the public API surface instead would be too weak in the other
# direction: a VM-internals change (say, a scheduler fix) alters the runtime's
# behavior while leaving every export identical, and a stale runtime would ship.
#
# So gate on freshness, not on bytes: fingerprint the INPUTS that determine the
# runtime and record them next to it. Any source change since the last vendoring
# is caught, on any host, without building anything. Same idea as
# website/og-manifest.json.
#
# Usage:
#   scripts/check-web-runtime-fresh.sh            # verify (CI)
#   scripts/check-web-runtime-fresh.sh --write    # record (after vendoring)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="$ROOT/crates/sema/web-runtime.lock"
MODE="${1:-check}"

cd "$ROOT"

# The crates that actually compile into the WASM are derived from
# `sema-wasm`'s own dependency tree, not globbed. Globbing `crates/*/src/*`
# would drag in every crate in the workspace — sema-mcp, sema-lsp, sema-dap,
# … — none of which reach the browser runtime, so any change to them would
# demand a pointless re-vendor of a ~5MB binary. Deriving the list also means
# it cannot go stale when a dependency is added or dropped.
WASM_CRATE_SPECS="$(cargo tree -p sema-wasm --edges normal --prefix none 2>/dev/null |
  grep -oE '^sema-[a-z-]+' | sort -u |
  while read -r crate; do printf 'crates/%s/src/*\ncrates/%s/Cargo.toml\n' "$crate" "$crate"; done)"
if [ -z "$WASM_CRATE_SPECS" ]; then
  echo "web-runtime freshness: could not resolve sema-wasm's dependency tree" >&2
  exit 1
fi

# The rest mirror the `file` recipes in jake/wasm.jake that vendor these assets.
# `.cargo/config.toml` and scripts/wasm-build.sh are in because they decide the
# rustflags the build compiles with.
#
# NOTE the trailing `/*` on every directory: a bare `crates/x/src` pathspec
# matches NOTHING in `git ls-files` (it wants files, not directory prefixes), so
# omitting it silently reduces the fingerprint and the check quietly stops
# covering the code. SANITY_FILES below fails loudly if that ever happens again.
# package-lock.json and jake/wasm.jake are inputs too: the vendor step copies
# @preact/signals-core and morphdom out of node_modules, so a dependency bump
# or a change to the copy list must invalidate the lock.
INPUT_PATHSPECS="$WASM_CRATE_SPECS
Cargo.toml
Cargo.lock
package-lock.json
jake/wasm.jake
packages/sema/src/*
packages/sema-web/src/*
.cargo/config.toml
scripts/wasm-build.sh"

DIGESTS="$(
  python3 - "$LOCK" "$MODE" <<PY
import hashlib
import json
import pathlib
import subprocess
import sys

specs = """$INPUT_PATHSPECS""".split()
root = pathlib.Path(".").resolve()


def digest_of(paths):
    """Order-independent over the file set, sensitive to path and content."""
    h = hashlib.sha256()
    for rel in sorted(paths):
        blob = pathlib.Path(rel).read_bytes()
        h.update(f"{rel}\0{len(blob)}\0".encode())
        h.update(blob)
    return h.hexdigest()


def tracked(*pathspecs):
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", *pathspecs],
        capture_output=True, text=True, check=True,
    ).stdout
    return [p for p in out.split("\0") if p]


inputs = tracked(*specs)
# A pathspec that quietly matches nothing turns this whole gate into a no-op, so
# assert that the files most likely to change the runtime are actually covered.
SANITY_FILES = [
    "crates/sema-wasm/src/lib.rs",
    "crates/sema-wasm/src/driver.rs",
    "crates/sema-vm/src/runtime/state.rs",
    "crates/sema-eval/src/eval.rs",
    "packages/sema-web/src/index.ts",
]
covered = set(inputs)
missing = [f for f in SANITY_FILES if f not in covered]
if missing:
    raise SystemExit(
        "web-runtime freshness: these inputs are not covered by the pathspecs, so "
        f"changes to them would not be detected: {missing}. Fix INPUT_PATHSPECS."
    )
assets = tracked("crates/sema/src/web/assets")
if not assets:
    raise SystemExit("web-runtime freshness: no tracked assets under crates/sema/src/web/assets")

print(json.dumps({"inputs": digest_of(inputs), "assets": digest_of(assets),
                  "input_count": len(inputs), "asset_count": len(assets)}))
PY
)"

INPUTS_NOW="$(printf '%s' "$DIGESTS" | python3 -c 'import json,sys; print(json.load(sys.stdin)["inputs"])')"
ASSETS_NOW="$(printf '%s' "$DIGESTS" | python3 -c 'import json,sys; print(json.load(sys.stdin)["assets"])')"

if [ "$MODE" = "--write" ]; then
  printf '%s\n' "$DIGESTS" | python3 -m json.tool >"$LOCK"
  echo "web-runtime freshness: recorded $LOCK"
  exit 0
fi

if [ ! -f "$LOCK" ]; then
  echo "::error::$LOCK is missing. Run 'jake wasm.web-runtime' and commit it." >&2
  exit 1
fi

INPUTS_WAS="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["inputs"])' "$LOCK")"
ASSETS_WAS="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["assets"])' "$LOCK")"

status=0
if [ "$INPUTS_NOW" != "$INPUTS_WAS" ]; then
  echo "::error::Embedded web runtime is stale: its sources changed since it was last vendored. Run 'jake wasm.web-runtime' and commit crates/sema/src/web/assets/ + crates/sema/web-runtime.lock." >&2
  echo "  recorded inputs: $INPUTS_WAS" >&2
  echo "  current  inputs: $INPUTS_NOW" >&2
  status=1
fi
if [ "$ASSETS_NOW" != "$ASSETS_WAS" ]; then
  echo "::error::Embedded web runtime assets were modified without re-vendoring (hand-edited, or the lock was not updated). Run 'jake wasm.web-runtime' and commit both." >&2
  echo "  recorded assets: $ASSETS_WAS" >&2
  echo "  current  assets: $ASSETS_NOW" >&2
  status=1
fi
[ "$status" -eq 0 ] && echo "embedded web runtime is current ✓"
exit "$status"
