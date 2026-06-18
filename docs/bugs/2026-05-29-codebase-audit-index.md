# Whole-Codebase Bug Audit — 2026-05-29

> **2026-06-18 re-triage:** every entry here (and the standalone bug docs) was re-verified against current HEAD; see **[2026-06-18-triage.md](./2026-06-18-triage.md)** for the authoritative status. Summary: 24 findings confirmed fixed, 28 still valid-as-written, 9 need re-planning, 1 stale. The adversarial re-check **re-opened 5** that had been marked fixed but were only masked/partial (notably VM-1 `CallNative` validation, the `time/parse` UTC bug, and the `eval.rs:773` UTF-8 slice).

**Status:** **All P0 and P1 findings fixed for 1.16.0** (2026-05-30); P2/P3 tracked below. See Resolution.
**Method:** 13 parallel reviewer agents, one per subsystem, hunting real bugs only
(correctness, panics, memory/UB, security, reliability). Cosmetic/structural concerns excluded.
**Scope:** All 12 crates, ~78k production LOC (`tests/` excluded).
**Confidence gate:** findings ≥80% reviewer confidence. Headline P0s re-verified against source.

This audit is a snapshot. Line numbers reflect the tree at commit `f609807` (v1.16.0).
Re-confirm each location before fixing — line numbers drift.

## Per-crate entries

| File | Crate | P0 | P1 | P2 | P3 |
|------|-------|----|----|----|----|
| [2026-05-29-sema-vm.md](./2026-05-29-sema-vm.md) | sema-vm | 1 | 3 | 2 | 1 |
| [2026-05-29-sema-llm.md](./2026-05-29-sema-llm.md) | sema-llm | 1 | 3 | 3 | – |
| [2026-05-29-sema-stdlib.md](./2026-05-29-sema-stdlib.md) | sema-stdlib | – | 6 | 4 | 2 |
| [2026-05-29-sema-core.md](./2026-05-29-sema-core.md) | sema-core | – | 1 | 1 | 1 |
| [2026-05-29-sema-reader.md](./2026-05-29-sema-reader.md) | sema-reader | – | 1 | – | 1 |
| [2026-05-29-sema-eval.md](./2026-05-29-sema-eval.md) | sema-eval | – | 2 | 1 | – |
| [2026-05-29-sema-lsp.md](./2026-05-29-sema-lsp.md) | sema-lsp | – | 1 | 2 | – |
| [2026-05-29-sema-dap-fmt.md](./2026-05-29-sema-dap-fmt.md) | sema-dap / sema-fmt | – | 1 | 1 | – |
| [2026-05-29-sema-notebook.md](./2026-05-29-sema-notebook.md) | sema-notebook | – | – | 2 | 1 |
| [2026-05-29-sema-wasm.md](./2026-05-29-sema-wasm.md) | sema-wasm | – | 1 | 3 | – |
| [2026-05-29-sema-binary.md](./2026-05-29-sema-binary.md) | sema (binary) | – | 1 | 1 | 2 |

## Top priorities

1. **`sema-llm/src/gemini.rs:36,80`** — API key in URL query string → leaks into reqwest
   error strings/logs; user-controlled `model` in path → SSRF. (P0)
2. **`sema-vm/src/vm.rs:1007-1020`** — `CallNative` native_id bounds check is `debug_assert!`
   only; crafted `.semac` panics in release. `validate_bytecode` has no `CallNative` arm. (P0)
3. **LLM secret/SSRF cluster** — key-in-URL (#1), key in `eprintln!`
   (`sema-llm/src/builtins.rs:3948`), unvalidated `base_url` (`sema-llm/src/openai.rs:647`).
4. **Trivially-triggerable user-input panics** — `bitwise` shift, `math/random-int` reversed
   bounds, `text/chunk` UTF-8 slice, `&display[..39]` in `error.rs`.
5. **Systemic sweeps below** fix the most findings per change.

## Systemic patterns (highest leverage)

### Pattern A — `i64 as usize` / `as u16` / `as i32` without a range guard
A user-supplied integer is cast to an unsigned/narrower type with no negativity or bounds
check, wrapping to a huge value → OOM allocation, panic, or silent wrong result.
The codebase already guards this correctly in `list/take`, `list/drop`, `string/repeat` —
the pattern is just applied inconsistently.

Sites: `sema-stdlib/src/list.rs` (repeat/chunk/split-at/sliding/page/times),
`sema-stdlib/src/string.rs:418` (pad), `sema-stdlib/src/server.rs:1194` (port `as u16`),
`sema-vm/src/vm.rs:1622` (NTH), `sema-wasm/src/lib.rs:363` (timeout `as i32`),
`sema-vm/src/emit.rs:45` and `compiler.rs:215` (`u16` index truncation).

**Sweep:** audit every `as usize`/`as u16`/`as u32`/`as i32` of a user-derived integer;
add a shared `as_index(value, name) -> Result<usize>` helper that rejects negatives.

### Pattern B — byte-index string slicing on UTF-8 → char-boundary panic
`&s[..N]` / `&s[s.len()-N..]` on a `String` panics when `N` lands inside a multi-byte char.
Sites: `sema-core/src/error.rs:310`, `sema-llm/src/builtins.rs:4238`,
`sema-stdlib/src/text.rs:362`. Related: the LSP char-index↔UTF-16 confusion family.

**Sweep:** a `truncate_chars(s, n)` / `floor_char_boundary` helper for the slice-panics;
a UTF-16 ↔ char-index utility for `sema-lsp`.

### Pattern C — untrusted `.semac` bytecode not fully validated
`validate_bytecode` validates some opcodes but lacks a `CallNative` arm and a per-instruction
stack-height verifier. Sites: `sema-vm/src/vm.rs:1007` (CallNative),
`sema-vm/src/vm.rs:790` (DUP), plus the documented `pop_unchecked` FIXME(C11).

### Pattern D — LLM secret / SSRF surface
API key in URL, API key in logs, user-controlled provider `base_url` with no host restriction.
Sites: `sema-llm/src/gemini.rs`, `sema-llm/src/builtins.rs:3948`, `sema-llm/src/openai.rs:647`.

### Pattern E — unbounded input → DoS
No parser recursion-depth limit (`sema-reader`), no HTTP request-body cap
(`sema-stdlib/src/server.rs:825`), no package-download size cap (`sema/src/pkg.rs:1023`).

## Coverage notes

- 13/13 reviewers returned. One self-rejected false positive (`serialize.rs:676` MakeClosure
  overflow, caught by an existing guard) was dropped.
- Re-verified against source: gemini key-in-URL, CallNative debug_assert, bitwise shift,
  HTTP body `usize::MAX`. The rest carry reviewer confidence and should be re-checked at fix time.
- Not deeply exercised: open-upvalue runtime under panic unwinding, full SSE/NDJSON multibyte
  stream reassembly, frontend JS.

## Resolution (2026-05-30, for 1.16.0)

Fixed across two passes. **Wave 0–4** (commits `f0a83b2`..`6e90380`) landed both P0s plus the
systemic sweeps; the **P1 completion pass** landed the remaining high-severity findings with a
red/green test each.

### P0 — both fixed
- **LLM-1** Gemini key-in-URL + model SSRF → header auth (`x-goog-api-key`) + validated `model`.
- **VM-1** `CallNative` bounds → real runtime check (no longer `debug_assert!`).

### P1 — all fixed
- **LLM-2** multibyte tool-result slice → `truncate_chars` (Pattern B sweep).
- **LLM-3** rate-limit clock underflow → `saturating_sub` (the thread::sleep runs on the sync
  caller thread, not inside `block_on`, so no runtime starvation in practice).
- **LLM-4** provider `base_url` SSRF → reject internal hosts when sandboxed (`is_internal_host`).
- **VM-2** `DUP` empty-stack UB → runtime guard returning `SemaError`.
- **VM-3** negative `i64`→`usize` in NTH; **VM-4** lambda rest-param shadowing → Pattern A sweep.
- **STD-1..5** bit-shift / random-int / UTF-8 slice / pad / list-fn casts → Pattern A & B sweeps.
- **STD-6** unbounded HTTP body → 16 MiB cap + `413`.
- **STD-7** `ws/close` no-op → shared `Rc<RefCell<Option<Sender>>>`, take+drop on close.
- **CORE-1** `&display[..39]` slice → `truncate_chars`. **READ-1** parser depth limit added.
- **EVAL-1/2** quasiquote vector/map correctness. **WASM-1** `timeout_ms` cast → `clamp_timeout_ms`.
- **LSP-1** char↔UTF-16 confusion → conversion in both directions (`char_col_to_utf16` /
  `utf16_to_char_col`), threaded through `span_to_range` and all range producers + the incoming
  `sema_col` sites; cross-file ranges use each file's source (now retained in `ImportCache`).
- **DAP-1** running-VM poll loop now answers `GetStackTrace`/`GetScopes`/`GetVariables`.
- **BIN-1** registry package-name traversal → `validate_package_spec` at install.

### Deferred to a later release (P2/P3)
Not blocking 1.16.0; still tracked in the per-crate files: VM-5 (scheduler panic-safety),
VM-6/VM-7 (const/cache-slot u16 overflow), LLM-6 (cache key vs model resolution), LLM-7 (cosine
dim mismatch), STD-8 (float→int saturation), STD-10 (`db/exec-batch` injection, by-design),
STD-11 (static-file symlink escape), STD-12 (`set_env` race), EVAL-3 (`eval_import` FS_READ gate),
LSP-2/LSP-3 (semantic-token UTF-16 length, named-let guard), FMT-1 (CRLF in string literals),
NB-1/NB-2 (notebook dup-id, no-auth writes), WASM-2/3/4, BIN-2/3/4 (git ref flags, download cap,
atomic extract).
