# Lisp Dialect Benchmark

How does Sema compare to other Lisp dialects on a real-world I/O-heavy workload? This page benchmarks fifteen Lisp dialects on the [1 Billion Row Challenge](https://github.com/gunnarmorling/1brc) — read weather-station measurements and compute min/mean/max per station. It is not a synthetic micro-benchmark; it exercises I/O, string parsing, hash-table accumulation, and numeric aggregation in a tight loop.

::: warning A benchmark ranks implementations, not just runtimes
Each dialect's **optimized** entry uses a comparable best effort — a hand-rolled integer×10 temperature parser and, where it helps the runtime, block/byte I/O. Even so, results partly reflect *how each program is written*, not pure runtime throughput. The [dialect notes](#dialect-notes) say where each number comes from; the [simple table](#simple-idiomatic) shows the same workload written the obvious way.
:::

## Benchmark

One same-machine run: **macOS 15.6, Apple M2 Max, native Homebrew runtimes, 10,000,000 rows (~124 MiB), best of 3, single-threaded.** Sema is the **v1.35.0 PGO build**. All fifteen implementations produce byte-identical output. PicoLisp is omitted — no native Homebrew formula.

### Optimized — best effort per dialect

Each implementation tuned to a comparable level (hand-rolled int×10 parser; block/byte I/O where the runtime benefits). Relative to the fastest (Fennel).

| Dialect           | Time (ms) | Relative | Runtime              |
| ----------------- | --------- | -------- | -------------------- |
| **Fennel/LuaJIT** | 501       | 1.0x     | JIT compiler         |
| **SBCL**          | 958       | 1.9x     | Native compiler      |
| **Racket**        | 1,387     | 2.8x     | JIT (Chez backend)   |
| **Chez Scheme**   | 1,441     | 2.9x     | Native compiler      |
| **Gambit**        | 2,049     | 4.1x     | Native compiler (C)  |
| **Guile**         | 2,055     | 4.1x     | Bytecode VM + JIT    |
| **Clojure**       | 2,664     | 5.3x     | JVM (JIT)            |
| **Sema**          | 3,462     | 6.9x     | Bytecode VM          |
| **Janet**         | 4,952     | 9.9x     | Bytecode VM          |
| **Chicken**       | 6,739     | 13.5x    | Native compiler (C)  |
| **Gauche**        | 7,051     | 14.1x    | Bytecode VM          |
| **Emacs Lisp**    | 7,941     | 15.9x    | Bytecode VM          |
| **ECL**           | 8,602     | 17.2x    | Native compiler (C)  |
| **newLISP**       | 8,935     | 17.8x    | Interpreter          |
| **Kawa**          | 17,822    | 35.6x    | JVM (JIT)            |

### Simple / idiomatic

The same workload written the obvious way in each dialect — built-in number parser, per-line I/O, standard data structures. No hand-rolled parsers, no block reads. Closer to "raw runtime on naive code." Relative to the fastest (Gambit).

| Dialect           | Time (ms) | Relative |
| ----------------- | --------- | -------- |
| **Gambit**        | 2,092     | 1.0x     |
| **Chez Scheme**   | 2,421     | 1.2x     |
| **Fennel/LuaJIT** | 2,680     | 1.3x     |
| **Clojure**       | 2,798     | 1.3x     |
| **SBCL**          | 3,025     | 1.4x     |
| **Guile**         | 5,009     | 2.4x     |
| **Sema**          | 6,278     | 3.0x     |
| **newLISP**       | 8,266     | 4.0x     |
| **Janet**         | 9,907     | 4.7x     |
| **Chicken**       | 10,641    | 5.1x     |
| **ECL**           | 13,239    | 6.3x     |
| **Emacs Lisp**    | 16,147    | 7.7x     |
| **Gauche**        | 16,484    | 7.9x     |
| **Kawa**          | 17,594    | 8.4x     |

The gap between the two tables is itself the story. Where optimized ≪ simple (Fennel, Racket, Guile, Janet, Gauche — and Sema at 1.8× between its entries), most of the win came from a hand-rolled parser and block/byte I/O. Where they're close (Clojure, newLISP), the runtime was already doing the work and there was little left to hand-tune.

## Dialect notes

What's behind each number — and which results are runtime ceilings versus implementation choices.

### Fennel / LuaJIT — the JIT runs away with it

Fennel compiled to LuaJIT is **the fastest entry, ahead of SBCL** (501 ms). LuaJIT's tracing JIT compiles the hot byte-scan loop to native code; with a `string.byte` integer parser and 1 MiB block reads it chews through ~250 MB/s. It's the clearest "runtime does the heavy lifting" result — but note its *simple* version is 2.7 s (5× slower), so the win is the optimized byte loop being unusually JIT-friendly, not a free lunch.

### SBCL — native code + `(safety 0)`

SBCL compiles to native machine code; in a type-specialized hot path there is no interpreter loop. With `(declare (optimize (speed 3) (safety 0)))`, block `read-sequence` I/O, an integer×10 parser, and in-place `setf` struct mutation, the inner loop runs near C speed. 25+ years of compiler work (descended from CMUCL). Its 1.3× → 1.0x optimization gain (simple 3.0 s → optimized 0.9 s) is the largest in the suite.

### Racket — byte I/O over the Chez backend

Racket reads 1 MiB byte blocks, scans for `;`/newline with O(1) `subbytes` slicing, and parses int×10. Its CS backend (Chez under the hood) plus byte strings put it third overall, just ahead of Chez itself — a notable result for a runtime usually thought of as "the teaching language."

### Chez Scheme — the other native compiler

Chez compiles to native code via a [nanopass framework](https://nanopass.org/). With a custom char-by-char parser and `make-hashtable`/`string-hash` it lands just behind Racket. The remaining gap to SBCL is mostly per-line string allocation versus SBCL's block parser.

### Gambit — compiled Scheme via C

`gsc` compiles Scheme to C to a native binary. It got the same int×10 parser as the other Schemes, but the win was negligible here — `read-line` + `substring` + string hashing dominate the loop, so I/O, not number parsing, is the bottleneck. Gambit 4.9.8 does lead the *simple* table: on naive code its runtime's line handling is the fastest of the fifteen.

### Clojure — JVM tax + warmup

Clojure's time includes JVM startup and JIT warmup, real costs for a single-shot script. `line-seq` + a transient map is idiomatic but not zero-cost, and `Double/parseDouble` handles the full IEEE-754 spec. Steady-state throughput is better than the wall-clock suggests; it trades raw speed for compactness.

### Guile — Scheme bytecode VM + JIT

Guile 3 has a bytecode VM with a native JIT (active on this platform — ~6× on a tight loop vs `GUILE_JIT_THRESHOLD=-1`). Its entry reads the whole file as one bytevector and scans bytes with an int×10 parser; the JIT compiles that scan loop well, putting it in a dead heat with Gambit's native binary (2,055 vs 2,049 ms) — a good showing for a decades-old runtime. Its earlier `read-line`-based entry ran 4.5 s; the byte rewrite is worth 2.2×.

### Janet — the closest architectural peer

Janet is the most architecturally comparable to Sema: an embeddable scripting language, bytecode VM, GC-based, no native compiler. Head to head, **Sema (3.5 s) lands ~1.4× ahead of Janet (5.0 s)** — a reversal of earlier editions of this benchmark, where Janet led by 1.6×. What flipped it: the July 2026 runtime work gave Sema the same tools Janet's implementation leans on — byte-oriented line folding (`bytes/*`, no UTF-8 navigation) and in-place mutable stat arrays — plus compiler work Janet's register VM doesn't need (last-use move semantics, a direct self-call opcode), and the August 2026 round cut fixed per-line overhead in Sema's fold pipeline. Janet's simple entry (9.9 s vs Sema's 6.3 s) keeps the same ordering on naive code. Still the comparison to watch.

### Chicken — compiled Scheme, I/O bound

Chicken compiles Scheme to C via `csc -O3` with an int×10 parser. This edition runs **CHICKEN 6.0.0** (released the week of the run), whose core string model moved to UTF-8 — and it paid for it here: 5.9 s on CHICKEN 5 became 6.7 s, the largest regression in the matrix. The remaining gap is per-line I/O allocation and Chicken's continuation-passing-style C ("Cheney on the MTA"), whose trampolining the C compiler can't fully optimize away.

### Gauche — byte scanning over UTF-8 strings

Gauche stores strings as **UTF-8 indexed by character**, so a `substring`/`string-index` implementation pays O(k) navigation per slice to convert character positions to byte offsets — a trap that can make a mature, well-engineered runtime look slow. The implementation here sidesteps it: read the whole file into a `u8vector`, scan **bytes** directly, parse int×10. That lands Gauche mid-pack at 7.1 s — and is a good reminder that on a char-indexed runtime, byte-oriented I/O is the difference between near-last and respectable.

### Sema — the fastest entry without a JIT

Sema (3.5 s) is the **fastest entry with no JIT and no native codegen** — everything above it in the table compiles to machine code somewhere (LuaJIT, SBCL, Racket's Chez backend, Chez, Guile's JIT, Gambit's C, the JVM). Earlier editions of this benchmark put Sema at the "interpreter floor" (8.1 s) — NaN-boxed immutable values, `Rc` reference counting, and no way to express the byte-oriented implementations the fast dialects use. The July 2026 performance work removed exactly those ceilings: `file/fold-lines-bytes` + `bytes/*` ops for byte scanning, `mutable-array` stats updated in place, an int×10 parse primitive, last-use move semantics in the compiler (`TakeLocal`) plus an owned-args callback protocol — so fold accumulators reach the copy-on-write gates with a unique reference and idiomatic immutable-update code mutates in place — and a direct self-call opcode. The same period also moved every callback onto the unified cooperative async runtime — a fold callback can `await` mid-stream, and blocking I/O overlaps across tasks — at a measured ~6% on this workload versus the peak pre-async measurement (3.6 s): per-line callbacks run as direct synchronous calls whenever a conservative bytecode analysis proves they cannot suspend, so only the chunked read handoff pays cooperative machinery (see [Performance Internals](./performance.md)). The August 2026 round then profiled what remained and cut fixed per-line costs — larger read chunks (8× fewer worker round-trips) and a fast path that stops reinstalling the task context on every native call — worth another ~10% here. The simple entry (6.3 s, the same naive code as the 8.1 s era) still reflects the byte-op/runtime wins on unchanged source.

### Emacs Lisp — buffer-based I/O

Emacs loads the whole file into a buffer with `insert-file-contents-literally` and parses int×10 directly from buffer characters with no substring extraction — strong for a venerable bytecode VM.

### ECL — Common Lisp via C

ECL compiles Common Lisp through C with `compile-file`, with an int×10 parser. The gap to SBCL is ECL's less aggressive native code generation.

### newLISP — a small, simple interpreter

newLISP's accumulation uses a hash, but on this 40-station dataset the data structure hardly matters — with so few stations even a linear scan is cheap, and per-row interpreter overhead dominates either way. A faithful picture of a deliberately minimal interpreter.

### Kawa — JVM Scheme, slower than expected

Kawa compiles Scheme to JVM bytecode. Even with Java interop (`BufferedReader`, `java.util.HashMap`), Scheme-on-JVM data representation, startup, and JIT warmup leave it last.

## What this benchmark doesn't show

This is one workload. Different benchmarks would reorder things:

- **CPU-bound computation** (fibonacci, sorting): the native compilers and JITs would pull further ahead; the I/O here amortizes some of the interpreter gap.
- **Startup time:** included in wall-clock but not isolated — it hits the JVM dialects (Clojure, Kawa) hardest.
- **Memory usage:** not measured; JVM runtimes carry a higher baseline than small standalone ones like Janet or Sema.
- **Multi-threaded:** Clojure, SBCL, Janet, and Guile can parallelize; Sema is single-threaded (its async/channels are cooperative, not parallel).
- **Developer experience:** Clojure's REPL, Racket's DrRacket, and SBCL's SLIME are far more mature than Sema's.

## Methodology

- **Dataset:** 10,000,000 rows (~124 MiB), 40 weather stations, from the [1BRC spec](https://github.com/gunnarmorling/1brc).
- **Environment:** macOS 15.6 / Apple M2 Max, native Homebrew runtimes (August 2026), all formulae refreshed to the latest released versions before the run — SBCL 2.6.7, Chez 10.4.1, CHICKEN 6.0.0, Gambit 4.9.8, Fennel 1.6.1 / LuaJIT 2.1, Clojure CLI 1.12.5.1664, Kawa 3.1.1, Racket 9.3, Guile 3.0.11, Gauche 0.9.15, Janet 1.41.2, ECL 26.5.5, Emacs 30.2, newLISP 10.7.5. Sema 1.35.0 (PGO). The exact versions are recorded in `benchmarks/1brc/results/native-macos-arm64/metadata.json`.
- **Measurement:** wall-clock, best of 3 consecutive runs per dialect, via `benchmarks/1brc/run-native-benchmarks.py` (all dialects measured together in one session). Sema is timed as the prebuilt PGO binary (`jake build-pgo`, run with `SEMA_SKIP_BUILD=1`).
- **Verification:** all fifteen implementations produce byte-identical normalized output (sorted stations, 1-decimal rounding) — checked every run.
- **Implementations:** each *optimized* entry uses a comparable best effort (hand-rolled int×10 parser; block/byte I/O where the runtime benefits); the *simple* table uses each dialect's naive idiom. PicoLisp is omitted (no native Homebrew formula).

### Reproducing

```bash
# Generate test data (or use benchmarks/data/bench-10m.txt)
python3 benchmarks/1brc/generate-test-data.py 10000000 benchmarks/data/bench-10m.txt

# Build the PGO Sema binary, then run the native matrix against it
jake build-pgo
SEMA_SKIP_BUILD=1 ./benchmarks/1brc/run-native-benchmarks.py benchmarks/data/bench-10m.txt
```

Implementation source: [`benchmarks/1brc/`](https://github.com/sema-lisp/sema/tree/main/benchmarks/1brc) (optimized) and [`benchmarks/1brc/simple/`](https://github.com/sema-lisp/sema/tree/main/benchmarks/1brc/simple) (simple/idiomatic).

<script setup>
import { onMounted } from 'vue'

onMounted(() => {
  document.querySelectorAll('table tr').forEach(row => {
    const firstCell = row.querySelector('td:first-child')
    if (firstCell && firstCell.textContent.trim().startsWith('Sema')) {
      row.classList.add('sema-row')
    }
  })
})
</script>

<style>
.sema-row {
  background: linear-gradient(90deg, rgba(245, 158, 11, 0.18), rgba(245, 158, 11, 0.06)) !important;
}
.sema-row td {
  font-weight: 600;
}
.sema-row td:first-child {
  border-left: 3px solid #f59e0b !important;
}
</style>
