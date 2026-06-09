# LSP Audit Harness — Design

**Status:** Approved design, awaiting implementation plan
**Date:** 2026-05-15
**Reviewed 2026-06-09:** Still unique work — NOT implemented by the Python suite (`crates/sema-lsp/tests/e2e/`, happy-path request/response tests) nor by `crates/sema-lsp/tests/lsp_e2e_test.rs` (capability smoke test). Before implementing, refresh: (a) extend the capability matrix with the five post-design capabilities (formatting, selectionRange, declaration, documentLink, callHierarchy); (b) account for `lsp_e2e_test.rs` as a third existing layer; (c) recheck the "code-lens round-trip is manual-only" claim against `test_code_lens.py`. Related: `2026-06-09-lsp-e2e-compliance-testing.md` covers the in-crate Rust protocol/lifecycle/spec-compliance suite; this design covers corpus/oracle-based correctness auditing — complementary, not duplicates.
**Goal:** Prove the `sema-lsp` server actually works end-to-end across all advertised capabilities, against real Sema code, before any new LSP features are added.

## Motivation

`sema-lsp` advertises 14 LSP capabilities, all with handlers and a 48-case Python e2e test suite. On paper it looks complete. In practice, "advertised + handler exists + happy-path test passes" is not the same as "works reliably in non-trivial real-world scenarios." LSP handlers commonly return `None` / `Vec::new()` on edge cases, error-recovery can mask bugs, and the only integration story for some code paths (notably the code-lens executeCommand → custom-notification round-trip) is manual editor testing.

The intent: a robust, mature, reliable LSP server that any editor — the six clients under `editors/` (vim, zed, emacs, helix, intellij, vscode) and any editor with native LSP support — can hook into, with confidence that everything in `examples/` works properly under all LSP capabilities.

## Approach

Build a torture-corpus harness driven by `pytest-lsp` (already in use). The harness runs invariant-based assertions across a matrix of (corpus file × capability × probe positions). Findings go into a structured punch list. Fixes are scoped from the findings as a separate body of work.

This phase explicitly does **not** add features. It produces:
1. The harness itself (permanent regression infrastructure).
2. Adversarial fixtures with expected-output siblings.
3. A findings list with triage.
4. An editor-verification layer (automated where high-value, documented elsewhere).

## Corpus

Two corpora with different invariants.

### Real-world corpus
Every `.sema` file under `examples/` (143 files). Treated as code that must work — any LSP failure here is a bug.

### Adversarial fixtures
New directory `crates/sema-lsp/tests/fixtures/` with curated pathological inputs. Each fixture has a sibling `.expected.json` declaring the diagnostics, symbols, or refs it should produce, so the assertion is exact rather than smoke.

Categories:
- Empty file, whitespace-only, comment-only
- Parse-error mid-file (tests error recovery → multiple diagnostics)
- Unbound reference, arity mismatch (compile-time diagnostic tests)
- Circular imports (a → b → a)
- Deep nesting (1000+ sexp levels)
- Unicode / multi-byte identifiers and strings (UTF-16 offset stress)
- Shadowing at every binding form: `define`, `let`, `let*`, `letrec`, `lambda`, `fn`, `defun`, `defn`, `for`, `match`, `try`, named-let, `do`
- Forward references in inner defines (regression for the nqueens.sema fix)
- Very long lines
- Very large file (10k+ lines)
- Cross-file imports with rename chains

## Invariants per capability

For each LSP capability, an assertion that's verifiable against the AST oracle or against the fixture's `.expected.json`.

| Capability | Real corpus | Adversarial fixtures |
|---|---|---|
| Parse diagnostics | Zero diagnostics for every example | Diagnostics match `.expected.json` exactly (count, span, severity) |
| Compile diagnostics | Zero warnings/errors | Specific unbound/arity errors match expected |
| Document symbols | Every `defun/defn/define/defmacro/defagent/deftool` in AST appears; counts match AST scan | N/A or empty for empty/broken files |
| Go-to-definition | Every identifier reference resolves to: local binding, imported def, or known builtin. Zero unresolved | Unresolved identifiers in error-recovery fixtures do not panic |
| References | For every top-level binding, LSP ref count == AST-derived ref count | Same |
| Hover | Every identifier returns non-empty hover (signature, doc, or "user-defined") | Same; broken-file hover does not panic |
| Completion | At end-of-identifier position, completion list contains that identifier | Completion at every probe position in broken file returns *something* (no panic, no hang) |
| Rename | For every local binding, rename produces source that re-parses cleanly and preserves binding count | Rename refused on builtins / special forms |
| Semantic tokens | Every identifier in source covered by a token (no gaps) | Same |
| Inlay hints | Every user-function call gets parameter hints matching declared params | Same |
| Folding | Every top-level sexp ≥2 lines produces a fold | Same |
| Document highlight | Symmetric: highlighting binding == highlighting reference returns same set | Same |
| Workspace symbols | Every `define`/`defun` across corpus reachable by exact-name query | Same |
| Code lens | Every top-level expression gets a "▶ Run" lens; executeCommand → `sema/evalResult` notification arrives within 5s | N/A |
| Signature help | Inside any function-call paren, signature help returns the function's arity / params | Same |

### Cross-cutting invariants

- **No panic, no hang.** Every capability called at every probe position of every fixture returns within a timeout, with a valid LSP response or a graceful empty.
- **Idempotence under no-op edits.** Sending the same document twice produces identical diagnostics / symbols.

## Harness architecture

Python + `pytest-lsp` — same toolchain as the existing e2e tests.

```
crates/sema-lsp/tests/
  e2e/                          # existing — focused per-capability happy-path tests, keep as-is
  fixtures/                     # NEW — adversarial fixtures + .expected.json siblings
  harness/                      # NEW
    conftest.py                 # pytest-lsp client fixture, corpus discovery
    invariants/
      diagnostics.py
      goto_definition.py
      references.py
      hover.py
      completion.py
      rename.py
      symbols.py
      semantic_tokens.py
      inlay_hints.py
      folding.py
      highlight.py
      code_lens.py
      signature_help.py
      no_panic.py
    ast_oracle.py               # walks `sema ast --json` output to derive ground-truth bindings/refs
    report.py                   # structured JSON + human summary
    test_corpus.py              # parametrized: (file × capability) → pass/fail
```

### AST oracle
For invariants like "reference count from LSP matches reference count from AST walk," ground truth must come from outside the LSP. The harness shells out to `sema ast --json` (already exists) and walks the JSON in Python to classify every identifier as binding or reference. No Rust-side changes needed.

### Probe-position strategy for "no panic"
Stratified sampling rather than every byte offset (millions of offsets for a 10k-line file): every identifier start, every `(`, every `)`, every string boundary, plus random offsets. Cap ~500 probes per file.

### CI integration
New `make test-lsp-harness` target. Runs in CI on every PR. Excluded from default `make test` because it's slow (~minutes for 143 files × N capabilities).

## Editor verification layer

The server-level harness covers ~95% of "does the LSP work." Editor-specific behavior (custom notifications, registration quirks, response interpretation) is the remaining 5%.

### Tier 1 — automated
- **vscode** — extend the extension's test setup with a small headless test using `@vscode/test-electron`. Loads a known fixture, asserts diagnostics render, code lens appears, `sema/evalResult` notification round-trips into the editor decoration. Estimated ~1 day.
- **helix** — verify `editors/helix/languages.toml` parses, command path resolves, basic capability advertisement. Scriptable as a CI smoke test. Estimated ~2 hours.

### Tier 2 — documented smoke procedures
- **intellij** — manual checklist: load plugin in dev IDE, open `examples/eliza.sema`, walk through (diagnostics show, ctrl-click works, rename works, etc.). Checklist lives at `editors/intellij/SMOKE.md`.
- **vim / emacs / zed** — these don't ship LSP clients themselves; users wire `sema lsp` into their own client. Document one tested config per editor in each `editors/<name>/README.md`, plus a smoke checklist.

### Rationale for the split
Automating intellij/vim/emacs/zed in CI is high-cost low-value. The harness already proves the server is correct; these tiers prove "we configured the client right," which is one-shot configuration, not ongoing risk.

## Punch list & fix workflow

Harness output → `docs/lsp-audit/` with two files:
- `findings.md` — human-readable, categorized: **bugs** (wrong/missing data), **gaps** (returns nothing where it should), **panics/hangs** (no-panic invariant violations), **performance** (>1s on adversarial fixture).
- `findings.json` — structured, for re-running and diffing across audit runs.

Each finding records:
- File path + offset that triggered it
- Capability + invariant violated
- Actual vs expected response
- Reproduction: minimal LSP request sequence

Triage classifies each finding by likely root cause and estimated effort. Fixes land in incremental PRs; harness re-runs as the regression check.

The audit deliverable is the harness + the findings list. Fixes are a separate body of work scoped from the findings.

## Done definition

Phase complete when:
1. Harness runs green on all 143 `examples/` files (zero invariant violations).
2. All adversarial fixtures pass (responses match `.expected.json`).
3. `make test-lsp-harness` integrated into CI.
4. vscode + helix automated smoke tests passing.
5. intellij/vim/emacs/zed smoke checklists documented and manually walked once.
6. `docs/lsp-audit/findings.md` exists and is empty, or every entry has a linked fix PR or explicit deferred-with-rationale.

This phase explicitly does **not** include new LSP features (formatting, code actions, etc.). Those are downstream of this phase.

## Non-goals

- Performance benchmarking under realistic load (deferred; orthogonal concern).
- New LSP capabilities of any kind.
- Refactoring `sema-lsp` internals beyond what specific findings demand.
- Editor plugin feature parity (e.g. making intellij match vscode's eval-result decorations).
