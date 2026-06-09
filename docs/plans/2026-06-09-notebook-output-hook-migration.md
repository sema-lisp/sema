# Notebook: migrate stdout capture from gag to sema-core output hook

**Date:** 2026-06-09
**Status:** Pending (small)
**Completes:** ADR #58 (thread-local writer hook — PARTIAL: hook shipped for DAP, notebook not migrated)

## Context

ADR #58 proposed replacing the notebook's `gag::BufferRedirect` (process-wide fd swap — races under concurrent evals, breaks under test harnesses, impossible on WASM) with an in-process writer hook. The hook has since shipped for the DAP server: `crates/sema-core/src/output_hook.rs` (`set_stdout_hook`/`set_stderr_hook`/`write_stdout`/`write_stderr`), and `sema-stdlib/src/io.rs` print functions already route through it.

The notebook still captures via gag: `crates/sema-notebook/src/engine.rs:~187` + `gag = "1.0.0"` in `crates/sema-notebook/Cargo.toml`.

## Tasks

1. In `engine.rs` cell evaluation: replace `gag::BufferRedirect::stdout()` with `set_stdout_hook` writing into a per-eval `Vec<u8>` buffer (mirror how `sema-dap/src/server.rs:~595-614` uses the hook); restore/clear the hook on all exit paths (success, error, panic-unwind if applicable).
2. Decide stderr handling: gag captured fd-level stderr; the hook has `set_stderr_hook` — wire it the same way if cell stderr is surfaced, otherwise drop stderr capture deliberately.
3. Remove the `gag` dependency from `crates/sema-notebook/Cargo.toml`.
4. Update ADR #58 status to implemented.

## Behavior change (intentional, per ADR #58)

Only output from Sema code (everything routed through `write_stdout`) is captured. Native code writing directly to `std::io::stdout()` bypasses the hook — correct for notebook semantics (user-program output, not interpreter chatter), but verify LLM streaming prints still behave sensibly in cells.

## Done When

- `cargo test -p sema-notebook` passes; notebook E2E (`make test-notebook-e2e`) shows cell stdout still rendered
- Two concurrent cell evals don't cross-capture output (the original gag failure mode)
- `gag` gone from the dependency tree
