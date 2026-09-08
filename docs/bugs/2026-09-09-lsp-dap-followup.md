# LSP and DAP follow-up — 2026-09-09

Base: `482720faed5aebbf964127c8d6d605d350df5a7f`. Tested on macOS arm64.
This is a functional correctness review, not a security assessment.

## Fixes

- DAP could lose a partial request when a debugger event interrupted its read.
  The framing parser now retains header and body progress across cancelled
  reads. A regression test interrupts every byte boundary, then reads the
  completed request and a second request. This follows Tokio's documented
  [cancellation-safety requirements](https://docs.rs/tokio/latest/tokio/macro.select.html#cancellation-safety).
- DAP compiled raw source without the macro expansion used by normal execution.
  Both async example sessions timed out before their first breakpoint. Launch
  now expands user and prelude macros, retains source spans, and captures
  expansion output as DAP events. The launched file also supplies the base path
  for relative loads.
- Top-level block locals were absent from debugger metadata. Compilation now
  retains their names and live ranges, with distinct slots across top-level
  forms. The regression checks both values and exclusion of out-of-scope names.
- Native DAP step-over entered cooperative callbacks because their separate VMs
  had shallower physical stacks. Native stepping now counts parked caller
  frames. Wire tests cover step-over and step-out after an async suspension.
  The headless debugger's existing frame-depth contract is unchanged.
- LSP could process a deferred close after a later reopen, or answer a deferred
  request using a later document version. Only workspace scan continuations may
  now yield to later messages.
- LSP disk-cache invalidation could remove an open document's semantic index.
  Closing an unsaved document could leave its old definitions in the index,
  even with a valid cached disk parse. Disk and open-document index handling
  now preserve the authoritative document and restore disk definitions on
  close. Tests cover normal and oversized-cache paths, deletion, and bad syntax.
- Rewriting unchanged file contents with a new modification time could hide
  workspace symbols. Verified matching content now refreshes cache and index
  timestamps.
- LSP registered file watching even when the client did not support dynamic
  registration. Initialization now checks the client's capability, as specified
  by the [LSP watched-file protocol](https://raw.githubusercontent.com/microsoft/language-server-protocol/gh-pages/_specifications/lsp/3.17/workspace/didChangeWatchedFiles.md).
- Builtin document highlights disappeared because builtins have no source-module
  owner. The fallback restores those highlights and excludes quoted data,
  local bindings, and later user redefinitions.

## Live DAP results

These sessions used a real `sema dap` child process through stdin/stdout, not an
in-process debugger mock. Each checked initialization, source breakpoints,
stack traces, variable evaluation, step-over, continuation, and disconnect.

| Program | Stop and inspection | Step-over | Completion |
| --- | --- | --- | --- |
| `examples/towers-of-hanoi.sema` | Line 87; `num-disks = 5`, initial towers | Line 89, not callback line 46 | DAP terminated; 31 moves |
| `examples/async-worker-pool.sema` | Worker line 33; `job = 1`, `id = 1` | Line 28 | DAP terminated; 12 jobs, sum 650 |
| `examples/async-pipeline.sema` | Pipeline stage line 24; `v = 1` | Line 21 | DAP terminated; 8 results |
| `sema-coder/tests/interrupt_test.sema` | Line 12; local `turn = <async-promise>`, `*busy* = #t` | Line 13 | Stopped before `done`; 5 checks, 0 failures |

### Reproduce

Run from the Rust repository. The driver uses only the Python standard library,
has per-request/event deadlines, and terminates and reaps its adapter child on
failure. It prints a JSON report and returns a failing status if an assertion
fails. Use `--verbose-output` to include all captured output events.

```sh
cargo build -p sema-lang

python3 scripts/dap-smoke.py --program examples/towers-of-hanoi.sema \
  --break-text '(define final-state' --evaluate num-disks --evaluate initial \
  --expect-evaluate num-disks=5 --expect-next-line 89 \
  --expect-output 'Total moves: 31'

python3 scripts/dap-smoke.py --program examples/async-worker-pool.sema \
  --break-text '(channel/send results' --evaluate job --evaluate id \
  --expect-evaluate job=1 --expect-evaluate id=1 --expect-next-line 28 \
  --expect-output 'sum of results =  650'

python3 scripts/dap-smoke.py --program examples/async-pipeline.sema \
  --break-text '((not (nil? v)) (channel/send out' --evaluate v \
  --expect-evaluate v=1 --expect-next-line 21 \
  --expect-output 'pipeline produced  8  items'

# Requires the separate sema-coder checkout next to the Rust repository.
python3 scripts/dap-smoke.py --program ../sema-coder/tests/interrupt_test.sema \
  --break-text '(handle-key {:kind :ctrl' --evaluate turn --evaluate '*busy*' \
  --expect-evaluate '*busy*=#t' --expect-next-line 13 \
  --finish-text '(done)' --finish-evaluate '*checks*' --finish-evaluate '*fails*' \
  --expect-finish '*checks*=5' --expect-finish '*fails*=0'
```

### Is sema-coder a good smoke test?

Its headless interruption test is a useful secondary test. It loads real
application code and checks Ctrl-C/Cmd-C task cancellation and idle input
handling, without a TTY, provider calls, or changes to that repository.

It should not be the only debugger smoke test:

- Loaded/imported code still produces the existing warning that it is not
  attached to the debugger. This run tests breakpoints in the launched test
  file, not inside the loaded application modules.
- The harness's `done` calls process exit. This smoke deliberately stops before
  it and inspects `*checks*` and `*fails*`; it does not establish a clean DAP
  termination handshake for an explicit process exit.
- Calling `(async/pending? turn)` in debugger evaluation returns
  `debug evaluation cannot suspend`. Inspecting `turn` itself works. Suspending
  watch expressions are outside the current evaluation contract.
- The full application has TTY, provider/configuration, and MCP startup paths.
  Those were not exercised. No external provider requests were made.

## Verification

- Targeted LSP/DAP Rust tests: **368 passed**, including a repeat in the main
  checkout with `cargo nextest run -p sema-lsp -p sema-dap`.
- Python LSP protocol suite: **56 passed**, with four existing client capability
  warnings about Markdown completion documentation.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p sema-lsp -p sema-dap -p sema-vm --all-targets -- -D warnings`:
  passed.
- `cargo clippy --workspace -- -D warnings`: passed during this follow-up.
- Full workspace nextest run before the last LSP follow-up tests: **7,879
  passed, 2 failed, 57 skipped**. Both failures were already reproduced on
  unchanged `main` during the preceding build-profile work:
  - `map_callback_async_test::hashmap_nan_entries_survive_every_async_map_traversal`
    expects NaN map keys that the runtime rejects.
  - `test_define_record_type_mutator_ignored` expects an ignored three-part field
    specification that the runtime rejects.

The serialized bytecode format is unchanged. No editor extension release is
needed for these implementation-only fixes.

All four live smoke sessions also passed after rebuilding the main checkout.
The temporary worktree and its build artifacts were removed. A cache sweep
overlapped the final test rebuild, which failed on missing artifacts; the
repeat with cleanup stopped passed. Run cache sweeps after builds finish.
