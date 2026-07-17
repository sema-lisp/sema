#!/usr/bin/env bash
# Profile-Guided Optimization build of the `sema` binary.
#
# Pipeline: instrument -> train (compute benches + a 1BRC sample) -> merge -> rebuild.
# Measured win on the bench suite: 1BRC -25%, compute -11..40% (see docs/performance-roadmap.md).
#
# Two modes:
#   ./scripts/pgo-build.sh            # full pipeline, leaves an optimized ./target/<profile>/sema
#   ./scripts/pgo-build.sh --profile-only   # instrument + train + merge, emit the .profdata only
#
# The emitted profile (target/pgo/merged.profdata) is what CI consumes via
# `RUSTFLAGS="-Cprofile-use=..."` for a single-pass release build (no per-runner
# training needed, works even when cross-compiling).
#
# Env knobs: PROFILE (release|dist, default release), TRAIN_ROWS (default 2000000),
#            PGO_DIR (default target/pgo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROFILE="${PROFILE:-release}"
PGO_DIR="${PGO_DIR:-$ROOT/target/pgo}"
PROFRAW_DIR="$PGO_DIR/profraw"
PROFDATA="$PGO_DIR/merged.profdata"
BENCH_DIR="examples/benchmarks"
TRAIN_ROWS="${TRAIN_ROWS:-2000000}"
TRAIN_DATA="$PGO_DIR/train-1brc.txt"
PROFILE_ONLY=0
[[ "${1:-}" == "--profile-only" ]] && PROFILE_ONLY=1

# llvm-profdata ships with the `llvm-tools` component; it lives in the rustlib
# bin dir, not on PATH.
find_profdata() {
  if command -v llvm-profdata >/dev/null 2>&1; then
    command -v llvm-profdata
    return
  fi
  local sysroot host cand
  sysroot="$(rustc --print sysroot)"
  host="$(rustc -vV | sed -n 's/host: //p')"
  cand="$sysroot/lib/rustlib/$host/bin/llvm-profdata"
  if [[ -x "$cand" ]]; then
    echo "$cand"
    return
  fi
  echo "ERROR: llvm-profdata not found. Run: rustup component add llvm-tools-preview" >&2
  exit 1
}
PROFDATA_BIN="$(find_profdata)"

BIN="$ROOT/target/$PROFILE/sema"
profile_flag() { [[ "$PROFILE" == "release" ]] && echo "--release" || echo "--profile $PROFILE"; }

rm -rf "$PROFRAW_DIR" "$PROFDATA"
mkdir -p "$PROFRAW_DIR"

echo "==> [1/4] instrumented build ($PROFILE)"
# shellcheck disable=SC2046 # profile_flag emits multiple words on purpose
RUSTFLAGS="-Cprofile-generate=$PROFRAW_DIR" cargo build $(profile_flag) --bin sema

echo "==> [2/4] training"
if [[ ! -f "$TRAIN_DATA" ]]; then
  if command -v python3 >/dev/null 2>&1; then
    python3 benchmarks/1brc/generate-test-data.py "$TRAIN_ROWS" "$TRAIN_DATA"
  else
    echo "   (python3 unavailable: training on compute benches only)"
  fi
fi
if [[ -f "$TRAIN_DATA" ]]; then
  "$BIN" "$BENCH_DIR/1brc.sema" -- "$TRAIN_DATA" >/dev/null
  # The byte-oriented 1BRC solution and the naive tier exercise different hot
  # paths (bytes/* + mutable-array vs string/split + assoc + string->number);
  # train all three so none of them is laid out cold in the release binary.
  "$BIN" benchmarks/1brc/1brc.sema -- "$TRAIN_DATA" >/dev/null
  "$BIN" benchmarks/1brc/simple/1brc.sema -- "$TRAIN_DATA" >/dev/null
fi
# Compute / closure / string / data / exception workloads exercise the dispatch loop.
for b in tak nqueens deriv upvalue-counter closure-storm higher-order-fold \
  hashmap-bench bench-features string-pipeline mandelbrot throw-catch; do
  [[ -f "$BENCH_DIR/$b.sema" ]] && "$BIN" "$BENCH_DIR/$b.sema" >/dev/null 2>&1 || true
done

echo "==> [3/4] merge profiles ($(ls "$PROFRAW_DIR"/*.profraw 2>/dev/null | wc -l | tr -d ' ') raw files)"
"$PROFDATA_BIN" merge -o "$PROFDATA" "$PROFRAW_DIR"
echo "    profile: $PROFDATA ($(du -h "$PROFDATA" | cut -f1))"

if [[ "$PROFILE_ONLY" == "1" ]]; then
  echo "==> profile-only: skipping optimized rebuild. Consume with:"
  echo "    RUSTFLAGS=\"-Cprofile-use=$PROFDATA\" cargo build $(profile_flag) --bin sema"
  exit 0
fi

echo "==> [4/4] optimized build (profile-use)"
# shellcheck disable=SC2046 # profile_flag emits multiple words on purpose
RUSTFLAGS="-Cprofile-use=$PROFDATA -Cllvm-args=-pgo-warn-missing-function" cargo build $(profile_flag) --bin sema
echo "PGO build complete: $BIN"
