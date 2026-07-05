# Benchmarks (hyperfine) + profiling (samply). Namespaced as `bench`.

runs = "10"
warmup = "3"
suite = "all"

# The bytecode VM is the sole evaluator; `bench` == the VM suite.
@group bench
@desc "Run the benchmark suite (params: suite runs warmup)"
@needs hyperfine "brew install hyperfine"
task bench suite="all" runs="10" warmup="3": [release]
    ./scripts/bench.sh --suite {{suite}} --runs {{runs}} --warmup {{warmup}}

@group bench
@desc "Run benchmarks and export a per-commit JSON snapshot"
@needs hyperfine "brew install hyperfine"
task bench-save suite="all" runs="10" warmup="3": [release]
    mkdir -p target/bench
    ./scripts/bench.sh --suite {{suite}} --runs {{runs}} --warmup {{warmup}} \
        --export target/bench/bench-$(git rev-parse --short HEAD 2>/dev/null || echo nogit).json

@group bench
@desc "Benchmark the closure suite"
@needs hyperfine "brew install hyperfine"
task bench-closure runs="10" warmup="3": [release]
    ./scripts/bench.sh --suite closure --runs {{runs}} --warmup {{warmup}}

@group bench
@desc "Benchmark the numeric suite"
@needs hyperfine "brew install hyperfine"
task bench-numeric runs="10" warmup="3": [release]
    ./scripts/bench.sh --suite numeric --runs {{runs}} --warmup {{warmup}}

@group bench
@desc "Export a baseline snapshot for later comparison"
@needs hyperfine "brew install hyperfine"
task bench-baseline runs="10" warmup="3": [release]
    mkdir -p target/bench
    ./scripts/bench.sh --runs {{runs}} --warmup {{warmup}} --export target/bench/baseline.json

@group bench
@desc "Benchmark and compare against target/bench/baseline.json"
@needs hyperfine "brew install hyperfine"
task bench-compare runs="10" warmup="3": [release]
    mkdir -p target/bench
    ./scripts/bench.sh --runs {{runs}} --warmup {{warmup}} \
        --export target/bench/current.json --compare target/bench/baseline.json

# ── 1BRC size ladder ─────────────────────────────────────────────────

@group bench
@desc "1BRC over 1M rows"
task bench-1m: [release]
    time ./target/release/sema examples/benchmarks/1brc.sema -- bench-1m.txt

@group bench
@desc "1BRC over 10M rows"
task bench-10m: [release]
    time ./target/release/sema examples/benchmarks/1brc.sema -- bench-10m.txt

@group bench
@desc "1BRC over 100M rows"
task bench-100m: [release]
    time ./target/release/sema examples/benchmarks/1brc.sema -- bench-100m.txt

# ── Profiling (samply) ───────────────────────────────────────────────

# Record a CPU profile of one benchmark. `jake bench.profile bench=tak`
@group profile
@desc "Record a samply CPU profile (params: bench)"
@needs samply "cargo install samply"
task profile bench="tak":
    mkdir -p target/profiles
    RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile release-with-debug -p sema-lang
    samply record --save-only --output target/profiles/{{bench}}-vm.json -- \
        ./target/release-with-debug/sema --no-llm examples/benchmarks/{{bench}}.sema
    echo "Profile saved: target/profiles/{{bench}}-vm.json"
    echo "Open with: samply load target/profiles/{{bench}}-vm.json"
