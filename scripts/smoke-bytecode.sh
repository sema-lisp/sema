#!/usr/bin/env bash
# Bytecode smoke test: compile, disassemble, and run all examples as .semac
# Usage: ./scripts/smoke-bytecode.sh [path-to-sema-binary]
set -euo pipefail

SEMA="${1:-./target/debug/sema}"
TIMEOUT="${BYTECODE_SMOKE_TIMEOUT:-15}"

if [ ! -x "$SEMA" ]; then
    echo "Error: sema binary not found at $SEMA"
    echo "Run 'cargo build' first."
    exit 1
fi

passed=0
failed_compile=0
failed_disasm=0
failed_run=0
skipped=0
total=0
failures=""

run_test() {
    local f="$1"
    local name
    name=$(basename "$f")
    total=$((total + 1))

    # Skip known problematic examples
    case "$name" in
        meta-eval-stress.sema)
            # Too slow for timeout-bounded smoke test
            skipped=$((skipped + 1))
            echo "  SKIP  $f (slow meta-eval)"
            return
            ;;
        web-server.sema|eliza-web.sema)
            # Starts a blocking server — cannot smoke test
            skipped=$((skipped + 1))
            echo "  SKIP  $f (starts server)"
            return
            ;;
        eliza.sema)
            # Reads from stdin in a loop — would block the smoke test
            skipped=$((skipped + 1))
            echo "  SKIP  $f (interactive)"
            return
            ;;
        game-of-life.sema)
            # Full-screen TUI — reads keys in a loop, no natural exit
            skipped=$((skipped + 1))
            echo "  SKIP  $f (interactive)"
            return
            ;;
        pico-*.sema)
            # Pico 2 hardware examples — require a serial port to a real device
            skipped=$((skipped + 1))
            echo "  SKIP  $f (requires serial-attached Pico hardware)"
            return
            ;;
        glados-downloads.sema)
            # Requires LLM API keys and PDF fixtures
            skipped=$((skipped + 1))
            echo "  SKIP  $f (requires LLM API keys)"
            return
            ;;
        http.sema)
            # External HTTP calls may timeout in CI
            skipped=$((skipped + 1))
            echo "  SKIP  $f (network-dependent)"
            return
            ;;
    esac

    # Per-example run-timeout overrides: correct but genuinely heavy in a debug
    # build, so the default bound is too tight (flaky under load) — keep full
    # compile/disasm/run coverage rather than skipping. math-and-crypto sieves
    # primes with a functional `cons`-based sieve; lists are vector-backed
    # (`cons` is O(n)), so the sieve is O(n^2) and takes ~13s debug at n=10000.
    local run_timeout="$TIMEOUT"
    case "$name" in
        math-and-crypto.sema)
            run_timeout=$((TIMEOUT < 45 ? 45 : TIMEOUT))
            ;;
    esac

    local semac="${f%.sema}.semac"

    # 1. Compile
    if ! "$SEMA" compile "$f" 2>/dev/null; then
        failed_compile=$((failed_compile + 1))
        failures="$failures\n  COMPILE  $f"
        echo "  FAIL  $f (compile)"
        return
    fi

    # 2. Disassemble (verify deserialization roundtrip)
    if ! "$SEMA" disasm "$semac" >/dev/null 2>&1; then
        failed_disasm=$((failed_disasm + 1))
        failures="$failures\n  DISASM   $f"
        echo "  FAIL  $f (disasm)"
        rm -f "$semac"
        return
    fi

    # 3. Run compiled bytecode
    if ! timeout "$run_timeout" "$SEMA" "$semac" >/dev/null 2>&1; then
        failed_run=$((failed_run + 1))
        failures="$failures\n  RUN      $f"
        echo "  FAIL  $f (run)"
        rm -f "$semac"
        return
    fi

    rm -f "$semac"
    passed=$((passed + 1))
    echo "  PASS  $f"
}

echo "=== Bytecode Smoke Test ==="
echo "Binary: $SEMA"
echo "Timeout: ${TIMEOUT}s per example"
echo ""

echo "--- examples/ ---"
for f in examples/*.sema; do
    [ -f "$f" ] && run_test "$f"
done

echo ""
echo "--- examples/stdlib/ ---"
for f in examples/stdlib/*.sema; do
    [ -f "$f" ] && run_test "$f"
done

echo ""
failed=$((failed_compile + failed_disasm + failed_run))
echo "=== Results ==="
echo "  Total:    $total"
echo "  Passed:   $passed"
echo "  Failed:   $failed (compile=$failed_compile, disasm=$failed_disasm, run=$failed_run)"
echo "  Skipped:  $skipped"

if [ -n "$failures" ]; then
    echo ""
    echo "Failures:"
    echo -e "$failures"
fi

if [ "$failed" -gt 0 ]; then
    exit 1
fi
echo ""
echo "All examples compiled, disassembled, and ran successfully!"
