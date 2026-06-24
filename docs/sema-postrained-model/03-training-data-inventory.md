# 03 — Training Data Inventory

Complete inventory of Sema codebase content that could serve as training data for a
post-trained model. Assessed on 2026-06-24.

---

## Summary

| Category | Files | Lines (Sema-relevant) | Quality | What it teaches |
|----------|-------|-----------------------|---------|-----------------|
| Example .sema files (unique) | 224 | 24,645 | ★★★★★ | Idiomatic code, all features, real programs |
| Eval test cases (`=> expected`) | 8 files | ~3,500 | ★★★★★ | Input→output pairs with ground truth |
| Integration/VM tests | 60+ files | ~31,445 | ★★★★☆ | Real programs, multi-feature, I/O |
| Reader unit tests | 1 file | 2,041 | ★★★★☆ | Syntax edge cases, lexer/parser |
| Prelude macros | 1 file | 216 | ★★★★★ | Macro definitions, idiomatic patterns |
| Website docs | 72 files | ~20,993 | ★★★★☆ | Syntax tutorials, 1,024 code blocks |
| sema-docs API entries | 826 files | 12,123 | ★★★★★ | Per-function docs, 1,156 executable examples |
| Internal docs/ | ~120 files | ~67,107 | ★★☆☆☆ | Design context, some code examples |
| Root markdown | 4 files | 2,112 | ★★★☆☆ | Feature overview, changelog examples |
| Stdlib Rust source | 36 files | ~600,000+ | ★★★☆☆ | Function signatures, type checking, errors |
| Error messages + hints | 1 file + 141 calls | 747 + hints | ★★★☆☆ | Error recovery, debugging guidance |
| Lower.rs (special form spec) | 1 file | 2,481 | ★★★★☆ | Semantic specification of all forms |
| Fuzz grammar | 1 file | 1,078 | ★★★☆☆ | Syntax stress testing |
| Playground examples | 68 files | 4,215 | ★★★★★ | Curated, self-contained demos |
| Formatter tests | 1 file | 1,103 | ★★★☆☆ | Formatting expectations |

### Estimated Total Unique Sema Code

- **Pure .sema files**: ~24,645 lines (224 unique files)
- **Embedded Sema in tests**: ~15,000+ lines of Sema snippets (~2,374 test cases)
- **sema-docs examples**: ~12,123 lines (826 entries, 1,156 executable examples)
- **Website docs code blocks**: ~5,000+ lines (1,024 `sema` code blocks)
- **Prelude**: 216 lines
- **Fuzz grammar**: 1,078 lines
- **Total**: **~58,000+ lines** of unique, high-quality Sema language content

---

## 1. Example .sema Files

### Primary examples/

| Subdirectory | Files | Lines |
|--------------|-------|-------|
| `examples/` (top-level) | 67 | — |
| `examples/stdlib/` | 25 | — |
| `examples/llm/` | 19 | — |
| `examples/benchmarks/` | 15 | — |
| `examples/providers/` | 10 | — |
| `examples/pi-sema/` | 9 | — |
| `examples/ai-tools/` | 6 | — |
| `examples/workflows/` | 1 | — |
| **Total** | **152** | **19,178** |

### Playground examples/

| Category | Files | Description |
|----------|-------|-------------|
| `getting-started/` | 7 | fibonacci, fizzbuzz, hello, quicksort, roman-numerals, towers-of-hanoi, advent-of-code |
| `functional/` | 6 | closures, comprehensions, huffman-coding, lazy-streams, map-filter, threading |
| `data/` | 10 | data-pipeline, emoji, hashmap-demo, json-api, regex-toolkit, sets, strings, text-processing, text-tools, unicode |
| `http/` | 5 | dad-jokes, exchange-rates, fetch-basics, ip-lookup, random-user |
| `patterns/` | 12 | bytecode-vm, dsl-builder, functional-patterns, interpreter, macros, meta-eval, multimethods, quickcheck, record-types, scheme-basics, state-machine |
| `concurrency/` | 7 | channels, fan-in, parallel-tasks, pipeline, real-sleep, timeout, worker-pool |
| `filesystem/` | 7 | delete-rename, directories, maze-generator, maze-solver, read-write, static-site, word-count |
| `math-crypto/` | 4 | datetime, math-crypto, matrix-math, pretty-print |
| `visuals/` | 10 | ascii-art, brainfuck, cellular-automata, game-of-life, l-system, logo-turtle, lorem-ipsum, mandelbrot, maze, perlin-noise |
| **Total** | **68** | **4,215 lines** |

### Deduplication Note

`benchmarks/1brc/sema-src/` contains 207 files that are copies of `examples/` and
`playground/examples/`. **Deduplicate before training** — only use the originals.

### Quality Assessment

★★★★★ — Highest quality. Real, idiomatic Sema code covering every feature: special forms,
closures, destructuring, pattern matching, LLM calls, async/concurrency, web servers, macros,
metaprogramming, stdlib usage. Files range from 5-line snippets to 767-line programs.

---

## 2. Test Files with Embedded Sema Code

### Eval Test Cases (Structured Input→Output Pairs)

These are the **highest-value** training data — each case is a Sema expression paired with its
expected evaluation result, providing ground-truth labels.

| File | Lines | Test Cases | Format |
|------|-------|------------|--------|
| `crates/sema/tests/eval_test.rs` | 1,577 | 219 | `eval_tests! { input => expected }` |
| `crates/sema/tests/eval_collections_test.rs` | 590 | 247 | `eval_tests!` |
| `crates/sema/tests/eval_stdlib_test.rs` | 370 | 182 | `eval_tests!` |
| `crates/sema/tests/eval_core_test.rs` | 449 | 177 | `eval_tests!` |
| `crates/sema/tests/eval_map_test.rs` | 359 | 112 | `eval_tests!` |
| `crates/sema/tests/eval_data_test.rs` | 119 | 67 | `eval_tests!` |
| `crates/sema/tests/eval_types_test.rs` | 160 | 66 | `eval_tests!` |
| `crates/sema/tests/eval_ergonomic_test.rs` | 56 | 25 | `eval_tests!` |
| **Total** | **3,680** | **1,095** | |

These `eval_tests!` macro cases are structured as `$input => $expected` — directly convertible
to instruction-tuning pairs:

```
Input:  "What does (+ 1 2) evaluate to in Sema?"
Output: "3"
```

### Integration / VM Tests

| File | Lines | Test Functions |
|------|-------|---------------|
| `crates/sema/tests/integration_test.rs` | 14,566 | 1,039 |
| `crates/sema/tests/vm_integration_test.rs` | 1,781 | 149 |
| `crates/sema/tests/vm_async_test.rs` | 1,209 | 91 |
| `crates/sema/tests/repl_display_test.rs` | 351 | — |
| `crates/sema/tests/doc_examples_test.rs` | 143 | — |
| 35+ other test files | ~10,000+ | — |

### Reader Unit Tests

| File | Lines | Tests |
|------|-------|-------|
| `crates/sema-reader/src/reader.rs` | 2,041 | 169 `#[test]` functions |

These tests cover syntax edge cases: regex literals (`#"..."`), f-strings, short lambdas
(`#(...)`), shebang lines, nested quoting, Unicode handling, dotted pairs, keyword parsing.

### Formatter Tests

| File | Lines |
|------|-------|
| `crates/sema-fmt/tests/formatter_test.rs` | 1,103 |

---

## 3. sema-docs API Documentation

The `sema-docs` crate generates structured per-function documentation with executable examples.
This is an exceptional training data source.

| Metric | Count |
|--------|-------|
| Total .md files | 826 |
| Total lines | 12,123 |
| Entries with `; =>` executable examples | 1,156 |

### Breakdown by Category

| Category | Docs |
|----------|------|
| `stdlib/lists` | 93 |
| `stdlib/strings` | 89 |
| `stdlib/math` | 70 |
| `special-forms` | 56 |
| `stdlib/file-io` | 45 |
| `stdlib/llm` | 44 |
| `stdlib/predicates` | 37 |
| `stdlib/maps` | 36 |
| `stdlib/system` | 31 |
| `stdlib/concurrency` | 29 |
| `stdlib/terminal` | 25 |
| `stdlib/streams` | 25 |
| `stdlib/bytevectors` | 23 |
| `stdlib/typed-arrays` | 16 |
| `stdlib/text-processing` | 16 |
| `stdlib/web-server` | 15 |
| `stdlib/context` | 15 |
| `stdlib/conversation` | 14 |
| `stdlib/vectors` | 13 |
| `stdlib/pio` | 13 |
| `stdlib/otel` | 12 |
| (20 more categories) | 118 |

### Quality Assessment

★★★★★ — Each entry has a function signature, description, and `; =>` executable example.
These are essentially **API reference training pairs** — perfectly structured for teaching
the model what each stdlib function does.

---

## 4. Website Documentation

| Category | Files | Lines |
|----------|-------|-------|
| `website/docs/` (root) | 15 | — |
| `website/docs/language/` | 3 | — |
| `website/docs/internals/` | 11 | — |
| `website/docs/llm/` | 15 | — |
| `website/docs/stdlib/` | 25 | — |
| `website/docs/tutorial/` | 3 | — |
| **Total** | **72** | **~20,993** |

### Code Blocks in Documentation

| Format | Count |
|--------|-------|
| ` ```sema ` blocks | 1,024 |
| ` ```scheme ` blocks | 124 |
| ` ```lisp ` blocks | 36 |
| **Total** | **1,184** |

### Quality Assessment

★★★★☆ — Website docs are highly curated with Sema code examples. Tutorial pages (basics,
functions, concurrency) are excellent for learning syntax. The 1,024 `sema` code blocks are
directly extractable as training examples.

---

## 5. Prelude Macros

| File | Lines |
|------|-------|
| `crates/sema-eval/src/prelude.rs` | 216 |

Contains macro definitions for: `->`, `->>`, `as->`, `some->`, `when-let`, `if-let`,
`with-stream`, `dotimes`, `for-range`, `with-span`, `with-session`, `defworkflow`, `phase`,
`agent`, `parallel`, `pipeline`, `async/pool-map`, `async/spawn-all`, `async/map`.

★★★★★ — Pure Sema source code (embedded as a string literal in Rust). Teaches macro
definition patterns and idiomatic Sema.

---

## 6. Stdlib Function Registrations

**522 `register_fn()` calls** across 36 module files in `crates/sema-stdlib/src/`.

| Module | Functions |
|--------|-----------|
| `list.rs` | 91 |
| `string.rs` | 88 |
| `math.rs` | 44 |
| `map.rs` | 30 |
| `io.rs` | 27 |
| `predicates.rs` | 26 |
| `typed_array.rs` | 23 |
| `stream.rs` | 20 |
| `async_ops.rs` | 19 |
| `text.rs` | 16 |
| (26 more modules) | 158 |

★★★★☆ — The Rust source shows exact arity, type checking, and error handling patterns. While
not directly Sema code, the function signatures and behavior descriptions can be extracted to
teach the model about the stdlib API.

---

## 7. Error Messages and Hints

| Source | Lines |
|--------|-------|
| `crates/sema-core/src/error.rs` | 747 |

### `with_hint()` calls across codebase: 141 total

Top sources: `map.rs` (33), `reader.rs` (19), `pio.rs` (15), `list.rs` (11), `vm.rs` (9).

The `veteran_hint` table maps Common Lisp / Clojure / Scheme names to Sema equivalents —
useful for training "translate from X to Sema" capabilities.

★★★☆☆ — Error messages teach what *not* to do and how to fix it. Useful for training error
recovery and debugging assistance.

---

## 8. Special Form Specification

| File | Lines | Content |
|------|-------|---------|
| `crates/sema-vm/src/lower.rs` | 2,481 | All special form lowering definitions |
| `crates/sema-core/src/value.rs` | 2,883 | Value types, Env, NativeFn |
| `crates/sema-core/src/context.rs` | 601 | EvalContext, callback mechanism |

★★★★☆ — `lower.rs` maps every special form (`define`, `fn`, `let`, `match`, `try`, etc.)
to its compiled form. This is the semantic specification of the language.

---

## 9. Markdown Code Blocks Across Entire Repo

| Pattern | Count |
|---------|-------|
| ` ```sema ` | 1,024 |
| ` ```scheme ` | 124 |
| ` ```lisp ` | 36 |
| **Total** | **1,184** |

These are spread across website docs, README, CHANGELOG, internal docs, and sema-docs entries.

---

## 10. Data Not Available

| Source | Status |
|--------|--------|
| REPL history / session transcripts | Does not exist |
| Formal BNF/EBNF grammar | Does not exist (syntax is implicit in reader.rs) |
| IntelliJ plugin Sema examples | No Sema content (Kotlin only) |
| User-generated code / community examples | Not in repo |

---

## Training Data Quality Ranking

For post-training a model on Sema, the sources ranked by training value:

1. **sema-docs API entries** (826 files, 1,156 examples) — structured per-function docs with
   executable examples. Perfect for instruction-tuning pairs.
2. **eval_tests! cases** (1,095 cases) — input→output ground truth pairs. Perfect for RFT
   grader and SFT.
3. **Example .sema files** (224 unique files, 24,645 lines) — idiomatic real programs covering
   all features.
4. **Playground examples** (68 files, 4,215 lines) — curated, self-contained, well-structured.
5. **Website docs code blocks** (1,024 blocks) — tutorial-style examples with explanations.
6. **Prelude macros** (216 lines) — macro definitions, idiomatic patterns.
7. **Reader tests** (169 tests) — syntax edge cases.
8. **Integration tests** (~1,279 test functions) — real programs, multi-feature.
9. **Error messages + hints** (141 hints) — debugging and error recovery.
10. **Lower.rs** (2,481 lines) — special form semantics (Rust, not Sema, but informative).
