# Fuzzing — cargo-fuzz (byte-level) + in-Sema grammar fuzzer. Namespaced `fuzz`.

@group fuzz
@desc "Install the nightly toolchain + cargo-fuzz"
task setup:
    rustup toolchain install nightly
    cargo install cargo-fuzz

@group fuzz
@desc "Run reader + eval byte-level fuzz targets"
task all: [reader, eval]
    echo "byte-level fuzzing done"

@group fuzz
@desc "Fuzz the reader (read + read_many)"
@needs rustup
task reader:
    cd crates/sema-reader && rustup run nightly cargo fuzz run fuzz_read -- -max_total_time=60
    cd crates/sema-reader && rustup run nightly cargo fuzz run fuzz_read_many -- -max_total_time=60

@group fuzz
@desc "Fuzz the evaluator"
@needs rustup
task eval:
    cd crates/sema-eval && rustup run nightly cargo fuzz run fuzz_eval -- -max_total_time=120 -timeout=10

# Grammar-based fuzzer written in Sema itself. Generates random *valid* programs
# and checks a printer/reader round-trip + a compiler/VM value oracle. No
# nightly needed. Every finding reproduces from one integer seed.
#   jake fuzz.grammar n=20000 depth=5 seed=123
@group fuzz
@desc "In-Sema grammar fuzzer (params: n depth seed)"
task grammar n="5000" depth="4" seed="": [release]
    @if eq({{seed}}, "")
        ./scripts/grammar-fuzz.sh check -n {{n}} -d {{depth}}
    @else
        ./scripts/grammar-fuzz.sh check -n {{n}} -d {{depth}} -s {{seed}}
    @end

@group fuzz
@desc "Print sample generated programs from the grammar fuzzer"
task grammar-emit n="5" depth="4": [release]
    ./scripts/grammar-fuzz.sh emit -n {{n}} -d {{depth}}
