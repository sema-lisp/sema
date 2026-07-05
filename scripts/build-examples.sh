#!/usr/bin/env bash
# Stress test: compile every runnable example into a STANDALONE BINARY with
# `sema build`, then execute that binary and check it succeeds. This exercises
# the whole release path — bytecode compile + serialize, import tracing, VFS
# bundling, runtime injection (libsui/ELF), and VM execution of a `.semac` from
# an embedded archive — not just `sema <file>`.
#
# Same blacklist as scripts/run-examples.sh (interactive/server/hardware), plus
# a per-binary run timeout. Exits non-zero if any example fails to build or run.
#
# Usage: scripts/build-examples.sh [--timeout SECONDS]
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
[ -x "$BIN" ] || { echo "building release..."; cargo build --release || exit 1; }

SKIP_FILES=(
  # interactive / server / hardware (same as run-examples.sh)
  "examples/web-server.sema" "examples/eliza-web.sema" "examples/eliza.sema"
  "examples/pico-blink.sema" "examples/pico-jukebox.sema" "examples/pico-midi.sema"
  "examples/pico-piano.sema" "examples/pico-show.sema" "examples/stdlib/io.sema"
  # not a deterministic build-path test:
  "examples/stdlib/http.sema"   # makes real network requests — flaky/non-
                                # deterministic; not exercising the build path.
  "examples/llm/async-stress-live.sema" # LIVE provider stress (real spend) —
                                # manual gate only, via `make llm-stress`.
)
is_skipped() { local f="$1"; for s in "${SKIP_FILES[@]}"; do [ "$f" = "$s" ] && return 0; done; return 1; }

OUTDIR="$(mktemp -d)"
trap 'rm -rf "$OUTDIR"' EXIT

GLOBS=( examples/*.sema examples/stdlib/*.sema )
built=0; ran=0; skipped=0; build_fail=(); run_fail=()
for f in "${GLOBS[@]}"; do
  [ -e "$f" ] || continue
  if is_skipped "$f"; then skipped=$((skipped+1)); continue; fi
  out="$OUTDIR/$(basename "${f%.sema}")"
  printf '  BUILD %-44s ' "$f"
  if ! "$BIN" build "$f" -o "$out" >/dev/null 2>"$OUTDIR/buildlog"; then
    echo "BUILD FAILED"; sed 's/^/        | /' "$OUTDIR/buildlog" | tail -4; build_fail+=("$f"); continue
  fi
  built=$((built+1))
  if timeout "$TIMEOUT" "$out" --no-llm >/dev/null 2>"$OUTDIR/runlog"; then
    echo "ok (built + ran)"; ran=$((ran+1))
  else
    code=$?
    if [ "$code" = "124" ]; then echo "RUN TIMEOUT"; else echo "RUN FAILED (exit $code)"; fi
    sed 's/^/        | /' "$OUTDIR/runlog" | tail -4; run_fail+=("$f")
  fi
done

echo ""
echo "=== build-examples: $ran built+ran, $skipped skipped, ${#build_fail[@]} build-fail, ${#run_fail[@]} run-fail ==="
[ ${#build_fail[@]} -gt 0 ] && printf '  BUILD FAILED: %s\n' "${build_fail[@]}"
[ ${#run_fail[@]} -gt 0 ] && printf '  RUN FAILED: %s\n' "${run_fail[@]}"
{ [ ${#build_fail[@]} -eq 0 ] && [ ${#run_fail[@]} -eq 0 ]; } || exit 1
echo "all runnable examples built into binaries and ran successfully"
