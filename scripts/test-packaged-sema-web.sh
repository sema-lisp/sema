#!/usr/bin/env bash
# Prove that the published sema-lang crate contains and embeds the complete
# browser runtime. The build runs from the unpacked .crate, not the checkout.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
SERVER_PID=""

if grep -R -n -E 'cfg\(web_runtime\)|jake wasm\.web-runtime' \
  "$ROOT/crates/sema/src" \
  "$ROOT/crates/sema/tests"; then
  echo "packaged web smoke: optional runtime configuration or end-user Jake guidance found" >&2
  exit 1
fi

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# No --offline: cargo package must rewrite the internal path dependencies
# (sema-core, sema-reader, ...) into pure registry version requirements and
# confirm they resolve, which needs the crates.io index. A fresh CI runner's
# local index cache has never been queried for these crates (a normal build
# resolves them via path, never touching the registry), so --offline fails
# here even though the crates themselves are published.
PACKAGE_TARGET="$TMP/package-target"
CARGO_TARGET_DIR="$PACKAGE_TARGET" cargo package \
  --manifest-path "$ROOT/Cargo.toml" \
  -p sema-lang \
  --allow-dirty \
  --no-verify

CRATE="$(find "$PACKAGE_TARGET/package" -maxdepth 1 -name 'sema-lang-*.crate' -print -quit)"
if [[ -z "$CRATE" ]]; then
  echo "packaged web smoke: cargo did not produce a sema-lang .crate" >&2
  exit 1
fi

mkdir -p "$TMP/unpacked"
tar -xzf "$CRATE" -C "$TMP/unpacked"
PACKAGE_DIR="$(find "$TMP/unpacked" -mindepth 1 -maxdepth 1 -type d -name 'sema-lang-*' -print -quit)"
if [[ -z "$PACKAGE_DIR" ]]; then
  echo "packaged web smoke: sema-lang package directory is missing" >&2
  exit 1
fi

# Single source of truth: every file git tracks under the runtime asset dir must
# survive packaging. No hand-maintained list to drift out of sync with the embed
# (src/web/runtime.rs embeds the same directory via rust-embed). Avoid `mapfile`
# so this runs on macOS's stock Bash 3.2, not just CI's Bash 4+.
TRACKED_ASSETS="$(git -C "$ROOT" ls-files -- crates/sema/src/web/assets)"
if [[ -z "$TRACKED_ASSETS" ]]; then
  echo "packaged web smoke: no runtime assets are tracked under crates/sema/src/web/assets" >&2
  exit 1
fi

# rust-embed walks the directory (not git), so anything sitting in the asset dir
# at build time is embedded by a local build — but `cargo package` ships only
# git-tracked files. A stray .DS_Store / editor backup, or a required asset
# someone forgot to `git add`, would then be embedded in dev yet MISSING from the
# .crate: the exact ship-vs-dev divergence. Reject any untracked/ignored file.
STRAY="$(git -C "$ROOT" status --porcelain --ignored -- crates/sema/src/web/assets)"
if [[ -n "$STRAY" ]]; then
  echo "packaged web smoke: untracked/ignored files in the asset dir (embedded locally, NOT shipped):" >&2
  echo "$STRAY" >&2
  exit 1
fi
while IFS= read -r tracked; do
  [[ -z "$tracked" ]] && continue
  rel="${tracked#crates/sema/src/web/assets/}"
  if [[ ! -s "$PACKAGE_DIR/src/web/assets/$rel" ]]; then
    echo "packaged web smoke: missing src/web/assets/$rel in $(basename "$CRATE")" >&2
    exit 1
  fi
done <<< "$TRACKED_ASSETS"

# Package manifests correctly replace workspace paths with registry versions.
# Patch those packages back to this checkout so the smoke remains runnable on
# unreleased commits while the sema-lang source itself stays the actual .crate.
mkdir -p "$PACKAGE_DIR/.cargo"
{
  echo '[patch.crates-io]'
  for crate in core reader vm eval stdlib llm fmt workflow lsp dap docs notebook mcp otel io; do
    printf 'sema-%s = { path = "%s/crates/sema-%s" }\n' "$crate" "$ROOT" "$crate"
  done
} >"$PACKAGE_DIR/.cargo/config.toml"

BUILD_TARGET="$TMP/build-target"
(
  cd "$PACKAGE_DIR"
  CARGO_TARGET_DIR="$BUILD_TARGET" cargo build --bin sema
)

# Prove the runtime is truly EMBEDDED, not read from the source tree at runtime:
# delete the assets from the built crate before serving. A binary with the bytes
# compiled in (rust-embed `debug-embed`) still serves; one that fell back to
# reading `CARGO_MANIFEST_DIR/src/web/assets` at runtime (the shipped-broken bug
# class) now fails here. This is the core invariant, enforced — not assumed.
rm -rf "$PACKAGE_DIR/src/web/assets"

printf '(display "packaged web runtime")\n' >"$TMP/app.sema"
PORT="$(python3 - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"

"$BUILD_TARGET/debug/sema" web "$TMP/app.sema" \
  --host 127.0.0.1 \
  --port "$PORT" \
  --no-open \
  >"$TMP/server.stdout" \
  2>"$TMP/server.stderr" &
SERVER_PID=$!

SHELL_HTML="$TMP/shell.html"
for _ in $(seq 1 100); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    wait "$SERVER_PID" || true
    SERVER_PID=""
    echo "packaged web smoke: sema web exited before serving" >&2
    cat "$TMP/server.stderr" >&2
    exit 1
  fi
  if curl -fsS "http://127.0.0.1:$PORT/" >"$SHELL_HTML" 2>/dev/null; then
    break
  fi
  sleep 0.1
done

if [[ ! -s "$SHELL_HTML" ]] || ! grep -q '<div id="app"></div>' "$SHELL_HTML"; then
  echo "packaged web smoke: application shell was not served" >&2
  cat "$TMP/server.stderr" >&2
  exit 1
fi

# Every embedded runtime file must actually be served — a missing or renamed JS
# module 404s in the browser even though the shell + wasm return 200. Serving
# `/` and the wasm alone is not enough.
while IFS= read -r tracked; do
  [[ -z "$tracked" ]] && continue
  rel="${tracked#crates/sema/src/web/assets/}"
  if ! curl -fsS "http://127.0.0.1:$PORT/__sema/$rel" >/dev/null 2>&1; then
    echo "packaged web smoke: embedded asset not served: /__sema/$rel" >&2
    cat "$TMP/server.stderr" >&2
    exit 1
  fi
done <<< "$TRACKED_ASSETS"

# The wasm must be served as `application/wasm` (browsers reject
# WebAssembly.instantiateStreaming otherwise) and be a real, non-truncated
# module. Use GET (-D -) rather than HEAD so it works regardless of HEAD support.
WASM_URL="http://127.0.0.1:$PORT/__sema/sema_wasm_bg.wasm"
CTYPE="$(curl -fsS -D - -o /dev/null "$WASM_URL" | tr -d '\r' \
  | awk -F': ' 'tolower($1)=="content-type"{print tolower($2)}')"
if [[ "$CTYPE" != application/wasm* ]]; then
  echo "packaged web smoke: wasm served with wrong content-type: '${CTYPE:-<none>}'" >&2
  exit 1
fi
curl -fsS "$WASM_URL" -o "$TMP/served.wasm"
WASM_MAGIC="$(head -c4 "$TMP/served.wasm" | od -An -tx1 | tr -d ' \n')"
if [[ "$WASM_MAGIC" != 0061736d ]]; then
  echo "packaged web smoke: served wasm is not a wasm module (magic=$WASM_MAGIC)" >&2
  exit 1
fi

echo "packaged web smoke: PASS"
