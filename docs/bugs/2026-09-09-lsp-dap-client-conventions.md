# LSP and DAP bug hunt, pass 2 — 2026-09-09

Base: `7e5c664d`. This pass follows the
[earlier fixes and real-program smoke tests](2026-09-09-lsp-dap-followup.md).
Scope: completion binding identity and DAP client coordinate/path conventions.

## Findings and fixes

| Finding | Cause | Fix and regression coverage |
| --- | --- | --- |
| DAP rejected a breakpoint on line zero for a zero-based client. | Initialization preferences were ignored; input validation always required line one or greater. | Convert client lines to internal one-based lines with overflow checks. Test zero, the largest representable line, and overflow. |
| DAP returned one-based positions to zero-based clients. | Breakpoint replies, breakpoint-change events, and stack frames serialized internal positions directly. | Convert every supported outgoing line/column field. Real-adapter tests check the reply, event, and stopped frame. |
| DAP returned native paths to clients requesting file URIs. | Stack-frame serialization ignored `pathFormat`. | Encode source paths as file URIs when requested. The wire test uses a filename containing spaces, `é`, and `%`. |
| Completion resolve showed builtin documentation for a user-defined or local `map`. | The local-completion loop replaced top-level items and discarded their source identity. Resolve also looked up builtin docs before checking the item's origin. | Retain top-level signatures and source data, remove shadowed builtin items, and do not attach builtin docs to lexical locals or source-owned definitions. Unit and editor-protocol tests cover both cases. |

DAP conversions follow the client's `linesStartAt1`, `columnsStartAt1`, and
`pathFormat` settings in the
[official DAP schema](https://raw.githubusercontent.com/microsoft/debug-adapter-protocol/main/debugAdapterProtocol.json).
Internal VM coordinates remain one-based. Default clients retain their existing
one-based coordinates and native paths. No VM execution or bytecode format
changes were needed in this pass.

## Verification

- The new regression tests reproduced incorrect completion documentation,
  rejected zero-based breakpoints, unchanged column bases, and native paths
  returned to URI clients before the corresponding fixes.
- `cargo build -p sema-lang`: passed; the wire tests used this rebuilt binary.
- `cargo nextest run -p sema-lsp -p sema-dap --no-fail-fast`: **374 passed**.
- LSP Python protocol suite: **58 passed**, including two new completion-resolve
  tests.
- `cargo clippy -p sema-lsp -p sema-dap --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Self-review of the changes: **Approve**. No unresolved correctness findings
  were found in the patch. No new dependencies were added.

Full-workspace tests were not rerun in this pass; the two baseline failures from
the earlier report remain outside this scope.

## Live DAP checks

All four earlier real-program sessions were repeated successfully:

- Hanoi: zero-based coordinates and URI paths; source line 89 was returned as
  client line 88 after step-over, then 31 moves and the DAP terminated event.
- Async worker pool: zero-based coordinates and URI paths; `job = 1`, `id = 1`,
  source line 28 returned as client line 27; 12 jobs and total 650, then terminated.
- Async pipeline: default conventions; eight results, then terminated.
- `sema-coder/tests/interrupt_test.sema`: default conventions; local task promise
  visible, five checks and zero failures at the breakpoint before `done`.

The smoke driver accepts `--zero-based` and `--uri-paths`. Its
`--expect-next-line` argument always uses the one-based source line, even when
the JSON protocol report contains zero-based positions. For example:

```sh
python3 scripts/dap-smoke.py --program examples/async-worker-pool.sema \
  --break-text '(channel/send results' --evaluate job --evaluate id \
  --expect-evaluate job=1 --expect-evaluate id=1 --expect-next-line 28 \
  --expect-output 'sum of results =  650' --zero-based --uri-paths
```

## Remaining items

The protocol suite still warns about Markdown documentation sent to a client
that did not advertise that format (seven warning instances, including the new
tests). Document-format negotiation was inspected but not changed in this pass.
The loaded-module, suspending watch-expression, and process-exit limitations
recorded in the earlier DAP report also remain.
