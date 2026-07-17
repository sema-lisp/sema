#!/usr/bin/env bash
#
# grammar-fuzz.sh — driver for the Sema grammar fuzzer (fuzz/grammar-fuzz.sema).
#
# Runs the in-language fuzzer, which checks two correctness oracles over randomly
# generated Sema programs:
#   * round-trip:  (= form (read (str form)))           — printer/reader symmetry
#   * value oracle: (= expected (eval form))            — compiler/VM correctness
# and detects hard VM crashes (panics; release is panic=abort) via a seed
# breadcrumb so every finding is reproducible from one integer seed.
#
# Usage:
#   scripts/grammar-fuzz.sh [check] [-n COUNT] [-d DEPTH] [-s SEED] [-v]
#   scripts/grammar-fuzz.sh emit  [-n COUNT] [-d DEPTH] [-s SEED] [-o FILE]
#
# Options:
#   -n COUNT   iterations / programs (default: 5000 for check, 20 for emit)
#   -d DEPTH   max generation depth (default: 4)
#   -s SEED    base seed (default: random)
#   -o FILE    emit mode: write programs to FILE instead of stdout
#   -v         verbose (check mode: also print passing forms)
#
# Exit status:
#   0  all checks passed
#   1  a deterministic mismatch was found (round-trip or value oracle)
#   2  a hard crash (VM panic) was found; reproducing seed is printed
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

MODE="check"
case "${1:-}" in
  check | emit)
    MODE="$1"
    shift
    ;;
esac

COUNT=""
DEPTH="4"
SEED=""
OUT=""
VERBOSE="0"

while getopts "n:d:s:o:v" opt; do
  case "$opt" in
    n) COUNT="$OPTARG" ;;
    d) DEPTH="$OPTARG" ;;
    s) SEED="$OPTARG" ;;
    o) OUT="$OPTARG" ;;
    v) VERBOSE="1" ;;
    *)
      echo "usage: $0 [check|emit] [-n COUNT] [-d DEPTH] [-s SEED] [-o FILE] [-v]" >&2
      exit 64
      ;;
  esac
done

# Locate (or build) the sema binary.
BIN=""
for cand in "$ROOT/target/release/sema" "$ROOT/target/debug/sema" "$(command -v sema 2>/dev/null || true)"; do
  if [ -n "$cand" ] && [ -x "$cand" ]; then
    BIN="$cand"
    break
  fi
done
if [ -z "$BIN" ]; then
  echo "==> building release binary (cargo build --release -p sema-lang)" >&2
  cargo build --release -p sema-lang || {
    echo "build failed" >&2
    exit 70
  }
  BIN="$ROOT/target/release/sema"
fi

# Default counts per mode.
if [ -z "$COUNT" ]; then
  if [ "$MODE" = "emit" ]; then COUNT="20"; else COUNT="5000"; fi
fi

# Random seed if not pinned.
if [ -z "$SEED" ]; then
  SEED=$((($(date +%s) ^ ($$ << 13) ^ ${RANDOM:-0}) % 1000000000))
fi

export SEMA_FUZZ_COUNT="$COUNT"
export SEMA_FUZZ_DEPTH="$DEPTH"
export SEMA_FUZZ_SEED="$SEED"
export SEMA_FUZZ_VERBOSE="$VERBOSE"

if [ "$MODE" = "emit" ]; then
  export SEMA_FUZZ_MODE="emit"
  [ -n "$OUT" ] && export SEMA_FUZZ_OUT="$OUT"
  exec "$BIN" "$ROOT/fuzz/grammar-fuzz.sema"
fi

# check mode: use a crash breadcrumb so a hard panic is still reproducible.
CRASH_FILE="$(mktemp)"
trap 'rm -f "$CRASH_FILE"' EXIT
export SEMA_FUZZ_CRASH_FILE="$CRASH_FILE"

"$BIN" "$ROOT/fuzz/grammar-fuzz.sema"
status=$?

if [ "$status" -eq 0 ]; then
  exit 0
elif [ "$status" -eq 1 ]; then
  # Deterministic mismatch; the sema program already printed reproduction info.
  exit 1
else
  # Hard crash (e.g. SIGABRT from panic=abort). Recover the in-flight seed.
  last="$(cat "$CRASH_FILE" 2>/dev/null || true)"
  echo "" >&2
  echo "CRASH: sema exited with status $status (likely a VM panic)" >&2
  if [ -n "$last" ] && [ "$last" != "ok" ]; then
    echo "  reproduce with: SEMA_FUZZ_SEED=$last SEMA_FUZZ_COUNT=1 SEMA_FUZZ_DEPTH=$DEPTH \\" >&2
    echo "                  $BIN $ROOT/fuzz/grammar-fuzz.sema" >&2
    echo "  or:             SEMA_FUZZ_MODE=emit SEMA_FUZZ_SEED=$last SEMA_FUZZ_COUNT=1 SEMA_FUZZ_DEPTH=$DEPTH \\" >&2
    echo "                  $BIN $ROOT/fuzz/grammar-fuzz.sema   # to see the offending program" >&2
  else
    echo "  (no breadcrumb captured; rerun with the same -s SEED to reproduce)" >&2
  fi
  exit 2
fi
