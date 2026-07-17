# Benchmarks (hyperfine) + profiling (samply). Namespaced as `bench`, so recipe
# names stay short: `jake bench.run`, `bench.save`, `bench.brc rows=10m`.

# The bytecode VM is the sole evaluator; the suite runs against it. `suite` picks
# the group (all | closure | numeric | …) — no separate per-suite recipes needed.
@group bench
@desc "Run the benchmark suite (params: suite=all|closure|numeric runs warmup)"
@needs hyperfine "brew install hyperfine"
task run suite="all" runs="10" warmup="3": [release]
    ./scripts/bench.sh --suite {{suite}} --runs {{runs}} --warmup {{warmup}}

@group bench
@desc "Run benchmarks and export a per-commit JSON snapshot"
@needs hyperfine "brew install hyperfine"
task save suite="all" runs="10" warmup="3": [release]
    mkdir -p target/bench
    ./scripts/bench.sh --suite {{suite}} --runs {{runs}} --warmup {{warmup}} --export target/bench/bench-$(git rev-parse --short HEAD 2>/dev/null || echo nogit).json

@group bench
@desc "Export a baseline snapshot for later comparison"
@needs hyperfine "brew install hyperfine"
task baseline runs="10" warmup="3": [release]
    mkdir -p target/bench
    ./scripts/bench.sh --runs {{runs}} --warmup {{warmup}} --export target/bench/baseline.json

@group bench
@desc "Benchmark and compare against target/bench/baseline.json"
@needs hyperfine "brew install hyperfine"
task compare runs="10" warmup="3": [release]
    mkdir -p target/bench
    ./scripts/bench.sh --runs {{runs}} --warmup {{warmup}} --export target/bench/current.json --compare target/bench/baseline.json

# 1BRC (billion-row challenge) size ladder — a recipe name can't start with a
# digit, so the size is a param: `jake bench.brc rows=1m|10m|100m`.
@group bench
@desc "1BRC benchmark over N rows (params: rows=1m|10m|100m)"
task brc rows="1m": [release]
    time ./target/release/sema examples/benchmarks/1brc.sema -- bench-{{rows}}.txt

# divan micro-benchmarks for the unified cooperative runtime's scheduler
# primitives (channel rendezvous, spawn/settle, timer arm+fire, HOF callback
# dispatch, idle drive turns, cancel_waiting sweeps). Separate from the
# hyperfine end-to-end suite above: `harness = false` on the `[[bench]]`
# target means `cargo bench` drives it directly (divan prints its own table).
@group bench
@desc "Run the divan scheduler micro-benchmark suite (sema-vm/benches/runtime_micro.rs)"
task micro:
    cargo bench -p sema-vm --bench runtime_micro

# ── Profiling (samply) ───────────────────────────────────────────────

# Record a CPU profile of one benchmark: `jake bench.profile bench=tak`
@group bench
@desc "Record a samply CPU profile (params: bench)"
@needs samply "cargo install samply"
task profile bench="tak":
    mkdir -p target/profiles
    RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile release-with-debug -p sema-lang
    samply record --save-only --output target/profiles/{{bench}}-vm.json -- ./target/release-with-debug/sema --no-llm examples/benchmarks/{{bench}}.sema
    echo "Profile saved: target/profiles/{{bench}}-vm.json"
    echo "Open with: samply load target/profiles/{{bench}}-vm.json"
