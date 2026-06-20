# Archived plans

Plans whose work has **shipped** or whose investigation **concluded** — kept for
historical context. The authoritative record of what landed is `CHANGELOG.md`,
`docs/adr.md`, and git history; these docs are the design thinking behind it.

Active (still-pending) plans live one level up in `docs/plans/`.

| Plan | Status |
| ---- | ------ |
| `2026-02-13-data-structure-crate-investigation.md` | Decided — `lasso` interning + `hashbrown` env/maps shipped (ADR #43/#44); the tree-walker hot path it targeted was retired. |
| `2026-02-16-compilation-strategy-investigation.md` | Built — recommended the stack bytecode VM, now Sema's sole evaluator (ADRs #50–#55). |
| `2026-05-15-adi-bytecode-verifier.md` | Shipped 1.17.0 — abstract stack-depth verifier (`verify_stack_balance`, ADR #56). |
| `2026-06-12-mcp-server.md` | Shipped 1.17.0 — `sema mcp` / `crates/sema-mcp`. |
| `2026-06-16-vm-module-loading.md` | Shipped — VM-native `load` (1.17.0) + `import` with module isolation (1.18.0). |
| `2026-06-18-retire-tree-walker.md` | Shipped 1.18.0 — bytecode VM is the sole evaluator. |
| `2026-06-18-retire-tree-walker-impl.md` | Shipped 1.18.0 — companion per-phase design. |
| `2026-06-18-c1-vm-hof-in-vm.md` | Shipped — HOF callbacks routed in-VM (commit `b75df47`). |
| `2026-06-09-notebook-output-hook-migration.md` | Shipped — notebook uses `set_stdout_hook`, dropped `gag`. |
| `2026-06-19-vm-web-worker-real-sleep.md` | Shipped 1.19.0 — real `async/sleep` in the playground worker. |
| `2026-06-09-lsp-followups-and-docs-research.md` | Superseded — docs audit replaced by the `sema-docs` crate. Three live stragglers (hover precedence, range formatting, DAP conditional breakpoints) were moved to `docs/wip.md` ("LSP / DAP follow-ups"). |
| `2026-06-09-spur-transmute-cleanup.md` | Shipped (commit `6f141e3`) — centralized `spur_to_bits`/`bits_to_spur` helpers replace all scattered `transmute::<u32, Spur>` sites; round-trip test + `size_of::<Spur>` const guard, no bench regression. |
| `2026-02-24-living-code-phase4.md` | Retired 2026-06-20 — Layer 6 runtime self-modification (`observe!`/`become!`/`evolve` etc.) killed for good; non-deterministic + unmaintainable, never ported off the retired tree-walker. PR #30 closed. See `docs/deferred.md` ("LC"). |
| `2026-06-20-docstrings-and-introspection.md` | Parked 2026-06-20 — the deterministic salvage (`doc`/`meta`) from Living Code layer 0. Confirmed clean to build (no source-text drag, binary path inherits it free), but cut for maintainability since doctests + LLM layers are gone. Revisit if a concrete need appears. |
| `vm-runtime-limitations.md` | Superseded 2026-06-20 — investigated VM↔tree-walker coupling for a slim `sema build` runtime; the tree-walker was retired in 1.18.0, so every claim is moot. Residual binary-size lever (cargo-feature-gate `pdf-extract`/LLM) is an optimization, not a limitation. |
| `vm-improvements.md` | Archived 2026-06-20 — 2026-03-10 VM improvement audit; 9/10 items shipped, #6 won't-fix, #10 (tracing GC) tracked as CORE-2 in `docs/deferred.md`. Current VM state: `docs/vm-status.md`. |
| `rust-idiomatic-review.md` | Archived 2026-06-20 — one-time style/security/architecture audit; deep findings shipped (stack-balance verifier ADR #56, §8 stabilization), residuals tracked in `docs/deferred.md` (VM-1 stack traces, C1 type-reflection follow-up). |
| `2026-06-07-performance-optimizations/` | Archived 2026-06-20 — alloc-reduction/unboxing/escape-analysis spike, never started; the shipped perf pass (PGO/LTO/string-opcodes, 1.19.2) was separate. 25-file OxCaml reference mirror deleted on archival; 3 original Sema docs kept. |
| `documentation-strategy-research.md` | Archived 2026-06-20 — LSP docs-delivery research; superseded by the `sema-docs` crate (structured source of truth). |
| `iteration-dsl.md` | Archived 2026-06-20 — CL-style `loop` DSL design; not adopted, simpler `dotimes`/`for-range` shipped instead. |
