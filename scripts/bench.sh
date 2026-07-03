#!/usr/bin/env bash
set -euo pipefail

SEMA_BIN="${SEMA_BIN:-./target/release/sema}"
BENCH_DIR="examples/benchmarks"

# ── Suite definitions ──────────────────────────────────────────────
suite_benchmarks() {
  case "$1" in
    core)      echo "tak nqueens deriv" ;;
    closure)   echo "upvalue-counter closure-storm higher-order-fold recursive-closure-churn" ;;
    data)      echo "hashmap-bench bench-features" ;;
    string)    echo "string-pipeline" ;;
    numeric)   echo "tak nqueens deriv mandelbrot" ;;
    exception) echo "throw-catch" ;;
    all)       echo "tak nqueens deriv upvalue-counter closure-storm higher-order-fold recursive-closure-churn hashmap-bench bench-features string-pipeline mandelbrot throw-catch" ;;
    *)         return 1 ;;
  esac
}

# ── Defaults ───────────────────────────────────────────────────────
MODE="vm"  # the bytecode VM is the sole evaluator
RUNS=10
WARMUP=3
EXPORT=""
COMPARE=""
SUITE="all"
BENCH_LIST=""

usage() {
  cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --suite SUITE         Predefined suite: core, closure, data, string, numeric,
                        exception, all (default: all)
  --bench NAME,...      Run specific benchmarks by basename (comma-separated)
                        e.g. --bench tak,mandelbrot
  --runs N              Number of timed runs (default: 10)
  --warmup N            Number of warmup runs (default: 3)
  --export FILE         Export unified results to JSON
  --compare FILE        Compare current run against a baseline JSON file
  -h, --help            Show this help

Suites:
  core        tak, nqueens, deriv (Gabriel-style)
  closure     upvalue-counter, closure-storm, higher-order-fold
  data        hashmap-bench, bench-features
  string      string-pipeline
  numeric     tak, nqueens, deriv, mandelbrot
  exception   throw-catch
  all         everything (default)

Examples:
  $0                                   # run all benchmarks
  $0 --suite core                     # core suite only
  $0 --bench tak,nqueens               # specific benchmarks
  $0 --export results.json             # save unified results
  $0 --export cur.json --compare base.json  # run and compare to baseline
EOF
  exit 1
}

# ── Argument parsing ───────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --suite)   SUITE="$2"; shift 2 ;;
    --bench)   BENCH_LIST="$2"; shift 2 ;;
    --runs)    RUNS="$2"; shift 2 ;;
    --warmup)  WARMUP="$2"; shift 2 ;;
    --export)  EXPORT="$2"; shift 2 ;;
    --compare) COMPARE="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown option: $1"; usage ;;
  esac
done

# ── Validation ─────────────────────────────────────────────────────
if [[ -n "$BENCH_LIST" && "$SUITE" != "all" ]]; then
  echo "Error: --bench and --suite cannot be used together"
  exit 1
fi

if ! suite_benchmarks "$SUITE" >/dev/null 2>&1; then
  echo "Error: unknown suite '$SUITE'. Choose from: core, closure, data, string, numeric, exception, all"
  exit 1
fi

if ! command -v hyperfine &>/dev/null; then
  echo "Error: hyperfine is not installed. Install with: brew install hyperfine"
  exit 1
fi

if [[ ! -x "$SEMA_BIN" ]]; then
  echo "Error: $SEMA_BIN not found. Run 'cargo build --release' first."
  exit 1
fi

# ── Resolve benchmark list ────────────────────────────────────────
if [[ -n "$BENCH_LIST" ]]; then
  IFS=',' read -ra BENCHMARKS <<< "$BENCH_LIST"
else
  read -ra BENCHMARKS <<< "$(suite_benchmarks "$SUITE")"
fi

# Deduplicate while preserving order
UNIQUE_BENCHMARKS=()
for b in "${BENCHMARKS[@]}"; do
  local_dup=false
  for u in "${UNIQUE_BENCHMARKS[@]+"${UNIQUE_BENCHMARKS[@]}"}"; do
    if [[ "$u" == "$b" ]]; then
      local_dup=true
      break
    fi
  done
  if [[ "$local_dup" == "false" ]]; then
    UNIQUE_BENCHMARKS+=("$b")
  fi
done
BENCHMARKS=("${UNIQUE_BENCHMARKS[@]}")

# ── Header ─────────────────────────────────────────────────────────
GIT_SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
TIMESTAMP=$(date '+%Y-%m-%dT%H:%M:%S')
echo "═══════════════════════════════════════════════════"
echo "  Sema Benchmarks"
echo "  git: $GIT_SHA  date: $TIMESTAMP"
echo "  mode: $MODE  runs: $RUNS  warmup: $WARMUP"
if [[ -n "$BENCH_LIST" ]]; then
  echo "  benchmarks: $BENCH_LIST"
else
  echo "  suite: $SUITE"
fi
echo "  bin: $SEMA_BIN"
echo "═══════════════════════════════════════════════════"
echo

# ── Temp dir for per-benchmark JSON ───────────────────────────────
TMPDIR_BENCH=$(mktemp -d)
trap 'rm -rf "$TMPDIR_BENCH"' EXIT

# ── Run benchmarks ────────────────────────────────────────────────
for name in "${BENCHMARKS[@]}"; do
  bench="$BENCH_DIR/${name}.sema"

  if [[ ! -f "$bench" ]]; then
    echo "Warning: $bench not found, skipping"
    continue
  fi

  echo ">>> $name"

  HYPERFINE_ARGS=(--runs "$RUNS" --warmup "$WARMUP" --style full --ignore-failure)
  HYPERFINE_ARGS+=(--export-json "$TMPDIR_BENCH/${name}.json")

  hyperfine "${HYPERFINE_ARGS[@]}" \
    -n "$name" "$SEMA_BIN --no-llm $bench"

  echo
done

# ── Unified JSON export ──────────────────────────────────────────
build_unified_json() {
  local output="$1"

  printf '{\n' > "$output"
  printf '  "git_sha": "%s",\n' "$GIT_SHA" >> "$output"
  printf '  "timestamp": "%s",\n' "$TIMESTAMP" >> "$output"
  printf '  "mode": "%s",\n' "$MODE" >> "$output"
  printf '  "runs": %d,\n' "$RUNS" >> "$output"
  printf '  "warmup": %d,\n' "$WARMUP" >> "$output"
  printf '  "benchmarks": {\n' >> "$output"

  local first=true
  for name in "${BENCHMARKS[@]}"; do
    local jsonfile="$TMPDIR_BENCH/${name}.json"
    [[ -f "$jsonfile" ]] || continue

    # Extract stats from the last result entry (vm in both mode, or the only one)
    local mean stddev min max
    mean=$(python3 -c "import json; d=json.load(open('$jsonfile')); r=d['results'][-1]; print(r['mean'])")
    stddev=$(python3 -c "import json; d=json.load(open('$jsonfile')); r=d['results'][-1]; print(r['stddev'])")
    min=$(python3 -c "import json; d=json.load(open('$jsonfile')); r=d['results'][-1]; print(r['min'])")
    max=$(python3 -c "import json; d=json.load(open('$jsonfile')); r=d['results'][-1]; print(r['max'])")

    if [[ "$first" == "true" ]]; then
      first=false
    else
      printf ',\n' >> "$output"
    fi
    printf '    "%s": {"mean": %s, "stddev": %s, "min": %s, "max": %s}' \
      "$name" "$mean" "$stddev" "$min" "$max" >> "$output"
  done

  printf '\n  }\n}\n' >> "$output"
}

if [[ -n "$EXPORT" ]]; then
  mkdir -p "$(dirname "$EXPORT")"
  build_unified_json "$EXPORT"
  echo "Results exported to $EXPORT"
fi

# ── Comparison mode ──────────────────────────────────────────────
if [[ -n "$COMPARE" ]]; then
  if [[ ! -f "$COMPARE" ]]; then
    echo "Warning: baseline file $COMPARE not found, skipping comparison"
  else
    CURRENT_JSON="$TMPDIR_BENCH/_current.json"
    build_unified_json "$CURRENT_JSON"

    echo "═══════════════════════════════════════════════════"
    echo "  Comparison vs $(basename "$COMPARE")"
    echo "═══════════════════════════════════════════════════"
    echo

    python3 -c "
import json

baseline = json.load(open('$COMPARE'))
current = json.load(open('$CURRENT_JSON'))

base_benchmarks = baseline.get('benchmarks', {})
cur_benchmarks = current.get('benchmarks', {})

all_names = list(dict.fromkeys(list(cur_benchmarks.keys()) + list(base_benchmarks.keys())))

for name in all_names:
    if name not in cur_benchmarks:
        print(f'  {name}: (not in current run)')
        continue
    if name not in base_benchmarks:
        print(f'  {name}: {cur_benchmarks[name][\"mean\"]*1000:.1f}ms (new, no baseline)')
        continue

    base_ms = base_benchmarks[name]['mean'] * 1000
    cur_ms = cur_benchmarks[name]['mean'] * 1000
    if base_ms > 0:
        pct = ((cur_ms - base_ms) / base_ms) * 100
    else:
        pct = 0.0

    if pct < -1:
        color = '\033[32m'  # green = faster
    elif pct > 1:
        color = '\033[31m'  # red = slower
    else:
        color = '\033[0m'

    sign = '+' if pct >= 0 else ''
    reset = '\033[0m'
    print(f'  {name}: {base_ms:.1f}ms \u2192 {cur_ms:.1f}ms ({color}{sign}{pct:.1f}%{reset})')
"
    echo
  fi
fi
