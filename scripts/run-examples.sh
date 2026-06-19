#!/usr/bin/env bash
# Run all runnable Sema examples headless and report pass/skip/fail.
#
# "Runnable" = pure, self-contained programs that complete on their own. We
# deliberately SKIP examples that would hang or can't run in CI:
#   - interactive (read stdin / REPL loops)
#   - long-running servers (http/serve)
#   - hardware targets (Raspberry Pi Pico)
#   - LLM / provider demos (need API keys + network)        [whole dirs]
#   - benchmark programs (need a generated multi-GB data file) [whole dir]
#
# Uses the release binary for speed. Each example gets a wall-clock timeout so a
# stray loop can't wedge the run. Exits non-zero if any *runnable* example fails.
#
# Usage: scripts/run-examples.sh [--timeout SECONDS]
set -u

TIMEOUT="${EXAMPLE_TIMEOUT:-30}"
while [ $# -gt 0 ]; do
  case "$1" in
    --timeout) TIMEOUT="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BIN="target/release/sema"
if [ ! -x "$BIN" ]; then
  echo "Release binary not found; building (cargo build --release)..."
  cargo build --release || exit 1
fi

# Per-file blacklist (interactive / server / hardware). Matched by path suffix.
SKIP_FILES=(
  "examples/web-server.sema"      # http/serve — never exits
  "examples/eliza-web.sema"       # http/serve — never exits
  "examples/eliza.sema"           # interactive read-line chatbot
  "examples/pico-blink.sema"      # Raspberry Pi Pico hardware
  "examples/pico-jukebox.sema"
  "examples/pico-midi.sema"
  "examples/pico-piano.sema"
  "examples/pico-show.sema"
  "examples/stdlib/io.sema"       # reads stdin
  "examples/stdlib/http.sema"     # real network requests — non-deterministic
                                  # (a flaky endpoint / 503 must not fail the
                                  # smoke run). Same rationale as
                                  # build-examples.sh. HTTP is covered by the
                                  # ignored integration suite (make test-http).
  "examples/glados-downloads.sema" # LLM demo: calls llm/extract + llm/auto-
                                  # configure, which hit a provider (e.g. Ollama
                                  # at localhost:11434). Passes only where a
                                  # provider is reachable; must not gate CI.
)

is_skipped() {
  local f="$1"
  for s in "${SKIP_FILES[@]}"; do [ "$f" = "$s" ] && return 0; done
  return 1
}

# Directories of runnable examples. (ai-tools/llm/providers need network+keys,
# pi-sema is hardware, benchmarks need a generated data file, fixtures/
# sema-web-app/notebook are not standalone programs — all excluded by omission.)
GLOBS=( examples/*.sema examples/stdlib/*.sema )

passed=0; skipped=0; failed=0; failures=()
for f in "${GLOBS[@]}"; do
  [ -e "$f" ] || continue
  if is_skipped "$f"; then
    echo "  SKIP $f"
    skipped=$((skipped+1))
    continue
  fi
  printf '  RUN  %-45s ' "$f"
  if out=$(timeout "$TIMEOUT" "$BIN" --no-llm "$f" 2>&1); then
    echo "ok"
    passed=$((passed+1))
  else
    code=$?
    if [ "$code" = "124" ]; then echo "TIMEOUT (${TIMEOUT}s)"; else echo "FAIL (exit $code)"; fi
    echo "$out" | tail -5 | sed 's/^/        | /'
    failures+=("$f")
    failed=$((failed+1))
  fi
done

echo ""
echo "=== examples: $passed passed, $skipped skipped, $failed failed ==="
if [ "$failed" -gt 0 ]; then
  printf '  FAILED: %s\n' "${failures[@]}"
  exit 1
fi
echo "all runnable examples passed"
