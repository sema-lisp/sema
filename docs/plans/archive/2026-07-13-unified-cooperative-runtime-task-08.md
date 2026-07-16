# Task 08: Legacy Deletion, Documentation, Examples, and Shipped Assets Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete every temporary runtime adapter and obsolete execution path,
make documentation/examples teach the final semantics, and regenerate and prove
all shipped browser/doc assets from package boundaries.

**Architecture:** Tasks 02–07 intentionally permit named migration bridges.
This layer removes them and turns the temporary baseline scanner into a
zero-legacy enforcement gate. Source docs, builtin docs, examples, generated
indexes, WASM/JS bundles, and embedded web assets are updated together so local,
packaged, and browser behavior describe and contain the same runtime.

**Tech Stack:** Rust source guards, Markdown, Sema examples, sema-docs generator,
wasm-pack, npm, Cargo packaging, Playwright.

## Execution contract

- **Status:** Ready only after Task 07 is accepted and committed.
- **Dependencies:** All production surfaces migrated, every adapter assigned to
  Task 08, complete inventory/resource/context/host matrices, browser tests.
- **Immutable inputs:** Master static-removal list, language semantics, shipped
  asset invariant from `AGENTS.md`, and no dual-path release policy.
- **Exact start state:** Clean worktree; latest commit subject is
  `refactor(runtime): unify native and browser hosts`; Tasks 01–07 are GREEN and
  every temporary row/symbol has Task 08 deletion ownership in the inventory and
  prior evidence. Task 1 materializes and verifies the exhaustive deletion
  checklist before deleting adapters.
- **Parallel work:** Adapter deletion is sequential under one integration owner.
  After source/API names stabilize, builtin/concept docs and examples/notebooks
  may be edited in parallel in disjoint files. Generation/package work starts
  only after source/docs/examples merge; independent review uses regenerated
  artifacts.

## Global constraints

- Tasks 01–07 and their independent reviews must be accepted before deletion.
- No compatibility bridge remains merely because deletion is inconvenient. A
  genuinely public alias may remain only if the master specification or Task 04
  named it as API compatibility rather than a migration mechanism.
- Do not retain old and new paths behind a feature flag, environment variable,
  target-specific fallback, file-existence check, or “temporary” dead code.
- Historical CHANGELOG entries remain historical. Add an Unreleased entry that
  explains the new behavior; do not rewrite prior release claims.
- Generated output is produced by repository commands, inspected, and committed.
  Do not hand-edit generated JSON, JS glue, or WASM.
- Anything used by shipped `sema web` is git-tracked, included in the `.crate`,
  embedded at build time, and tested after source assets are removed.
- Do not profile or benchmark in this layer.

---

## Files and responsibilities

**Create**

- `crates/sema-docs/entries/stdlib/concurrency/async-race-owned.md`.
- `crates/sema-docs/entries/stdlib/concurrency/async-with-timeout.md`.
- `website/docs/internals/cooperative-runtime.md` — runtime/host/resource model.
- `examples/async-observation-vs-ownership.sema` — practical API contrast.
- `docs/plans/evidence/unified-cooperative-runtime/task-08.md` — deletion,
  generation, package, and example evidence.
- `docs/plans/reviews/unified-cooperative-runtime/task-08.md` — independent
  deletion/docs/package review.

**Delete when inventory confirms their replacement**

- `crates/sema-core/src/async_signal.rs` — yield/resume/callback/TLS protocol.
- `crates/sema-vm/src/scheduler.rs` — migration adapter over the runtime.
- `docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline` —
  nonzero baselines are no longer permitted.
- Any browser replay/cache/sleep helper left in `crates/sema-wasm/src/lib.rs` or
  playground worker code.

**Modify**

- Core/VM/eval/stdlib module exports and Cargo dependencies after deletion.
- `scripts/check-unified-runtime-legacy.sh` — `--check` exits nonzero on any
  production match and prints exact path/line/symbol/reason.
- `crates/sema/tests/runtime_conformance_test.rs` — invoke zero-legacy guard and
  check worker/runtime/browser boundaries.
- All existing concurrency builtin entries, especially `async-all`,
  `async-race`, `async-timeout`, `async-spawn`, `async-spawn-all`, `async-map`,
  `async-pool-map`, `async-run`, `async-cancel`, and promise predicates.
- `crates/sema-docs/builtin_docs.generated.json` — generator output only.
- `website/docs/tutorial/concurrency.md`, `website/docs/stdlib/index.md`,
  `website/docs/internals/{bytecode-vm,evaluator}.md`, and relevant MCP/LLM docs.
- `README.md` and the top Unreleased section of `CHANGELOG.md`.
- `examples/async-*.sema`, `examples/async-everything.sema`, async notebooks,
  and `playground/examples/concurrency/*.sema`.
- `playground/src/examples.js` — generated by `playground/build.mjs` only.
- `packages/sema-wasm/pkg/*` and `crates/sema/src/web/assets/**` — generated by
  repository WASM/vendoring commands and committed where repository policy
  requires.
- Build/package scripts and manifests only where needed to keep generated
  runtime inputs tracked and unconditional.

## Task 1: Prove every temporary adapter has a deletion action

**Files:** runtime inventory, Task 02–07 evidence/reviews

- [ ] **Step 1: Extract all temporary rows and source symbols**

```bash
rg -n 'temporary|adapter|bridge|LegacyRuntimeBridge|delete in Task 08|Task 08' \
  docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  crates
scripts/check-unified-runtime-legacy.sh
```

- [ ] **Step 2: Build the deletion checklist in Task 08 evidence**

One row per symbol: exact definition, all callers, replacement, deletion diff,
and post-delete test. No “covered by scan” aggregate row.

- [ ] **Step 3: Delete adapters one subsystem at a time**

After each deletion run its focused Task 02–07 tests. Do not add replacement
compatibility wrappers to make source scans pass.

## Task 2: Install the zero-legacy and boundary guard

**Files:** scanner and runtime conformance test

- [ ] **Step 1: Write failing scanner fixtures**

Under the test’s temporary directory, prove the scanner catches each final
removed form: `IoHandle`, `IoPoll`, `YieldReason`, scheduler target/result,
yield setter, resume TLS, reentrant run helper, evaluator/spawn/cancel callback,
temporary running-task removal, runtime-task `block_on`, direct sleep/blocking
recv, nested drive, worker `Value`, replay marker/limit, sync XHR, Atomics wait,
and host-side scheduler loop.

- [ ] **Step 2: Implement exact allowlist format**

Permanent synchronous-only infrastructure exceptions use four columns: exact
file, exact symbol/pattern, proof it cannot execute in a runtime task, and owner.
Directories, globs, and substring-only exclusions are rejected by tests.

- [ ] **Step 3: Delete the baseline and require empty output**

```bash
scripts/check-unified-runtime-legacy.sh --check
cargo test -p sema-lang --test runtime_conformance_test
```

Expected: exit zero with no unapproved production match. Historical docs and
test fixtures are explicitly out of production scan scope.

## Task 3: Rewrite builtin and conceptual documentation

**Files:** sema-docs entries, website docs, README, CHANGELOG

- [ ] **Step 1: Document the practical distinction first**

Use one grounded scenario throughout: a Sema Coder root starts an index refresh.
`async/timeout` lets the UI stop waiting after 200 ms while refresh continues;
`async/with-timeout` owns a one-shot preview generation and cancels/reaps it when
the deadline wins. Explain why promise-taking operations observe and
thunk-taking operations own.

- [ ] **Step 2: Document all final contracts**

Cover detached lifetime, origin-root cancellation, all/race settlement order,
owned cleanup, conditions, `async/run`, multiple roots/shared globals, context
inheritance, resource classes, shutdown, WASM Promise behavior, and Stop.
Examples must say whether work continues after a timeout/race/failure. Resource
and LLM docs distinguish stopping local delivery/dispatching protocol-supported
cancellation from any unsupported claim that a remote provider stopped work or
billing.

- [ ] **Step 3: Generate and check builtin docs**

```bash
jake docs
jake docs-check
```

Expected: strict generator, clean generated diff check, and LSP coverage pass.

## Task 4: Rewrite examples and notebooks to express intent

**Files:** examples, notebooks, playground concurrency examples

- [ ] **Step 1: Classify every async pattern**

```bash
rg -n 'async/(all|race|timeout|spawn|map|pool-map|spawn-all)' \
  examples playground/examples website/docs crates/sema-docs/entries
```

For each match record observation, detached lifetime, or owned scope. Convert
structured fan-out to `spawn-all`/`map`/`pool-map`; cancellation deadlines to
`with-timeout`; loser-cleanup races to `race-owned`. Retain observational forms
only when continuation is intentional and demonstrate the later result/side
effect.

- [ ] **Step 2: Remove obsolete browser caveats**

Delete claims about virtual instant sleep, Atomics wait, replay, CPU chunks as
the only fairness mechanism, or timeout implicitly cancelling promises. Replace
with final bounded-quantum/resource behavior.

- [ ] **Step 3: Run examples and generate playground index**

```bash
cd playground && node build.mjs
cd ..
jake examples
jake example-notebook
jake example-notebooks-async
```

Expected: generated example index is current and every headless example passes.

## Task 5: Regenerate and verify WASM/web shipped assets

**Files:** WASM package, web assets, build/package scripts

- [ ] **Step 1: Generate from clean inputs**

```bash
jake wasm.build
jake wasm.js-lib-build
jake wasm.web-runtime
jake pg.build
```

Expected: commands succeed; inspect `git status` and include every required
tracked generated change. No required asset remains ignored/untracked.

- [ ] **Step 2: Run browser and web E2E gates**

```bash
jake test.playground-e2e
jake test.web-e2e
```

Expected: browser suites pass with Promise eval, exact-root Stop, fetch, output,
and multiple-root cases.

- [ ] **Step 3: Run the package-boundary proof**

```bash
scripts/test-packaged-sema-web.sh
```

Expected: a `.crate` is built/unpacked, runtime source assets are removed after
build, and the built binary still serves the complete embedded runtime. No
checkout path, generated-at-install step, or end-user dev command is required.

## Task 6: Full deletion/docs verification and independent review

- [ ] **Step 1: Run layer gates**

```bash
scripts/check-unified-runtime-legacy.sh --check
cargo test -p sema-lang --test runtime_conformance_test
cargo test --workspace
jake examples
jake smoke-bytecode
jake lint
jake docs-check
jake test.playground-e2e
jake test.web-e2e
scripts/test-packaged-sema-web.sh
git diff --check
```

Expected: all GREEN; no expected RED/ignored runtime migration test remains.

- [ ] **Step 2: Assign independent deletion/docs/package review**

Finding IDs use `UR-T08-R###`. Reviewer compares every Task 02–07 adapter row to
source, challenges scanner scope/allowlists, executes docs examples, compares
generated artifacts with sources, and inspects unpacked `.crate` contents.

- [ ] **Step 3: Fix every finding and regenerate from sources**

A generated-file finding is fixed in its source/generator, then all affected
outputs are regenerated. A missing scan match receives a scanner fixture.

- [ ] **Step 4: Commit the accepted layer**

```bash
git add -A crates docs website examples playground packages scripts \
  README.md CHANGELOG.md Cargo.toml Cargo.lock jake
git commit -m "docs(runtime): remove legacy paths and document final runtime"
```

Before committing, inspect `git diff --cached --name-status`; `git add -A` is
restricted to this dedicated member-repo worktree and is used here because the
task intentionally includes reviewed deletions and generated files.

## Completion criteria

- Every temporary adapter and legacy execution path is deleted.
- The zero-legacy scanner catches each forbidden class and has no broad allowlist.
- Builtin/conceptual docs state observation, ownership, cancellation, and
  multiple-root semantics consistently.
- Examples and notebooks use owned APIs when cleanup is intended and prove
  continuation when observation is intended.
- Generated docs, example index, WASM/JS, and embedded web assets are current.
- Browser, shipped `.crate`, examples, smoke, lint, docs, and workspace gates pass.
- Independent review and durable evidence are clean.

---

## Annotation (2026-07-15) — remaining work to delete the legacy scheduler

The `eval_str_compiled` full flip (the last legacy-VM eval entry) is still
DEFERRED; family A + `retry` parity are DONE (see task-03 annotation and
`docs/plans/evidence/unified-cooperative-runtime/red-baseline.md`). To flip
`eval_str_compiled` → `run_exprs_via_runtime`, then delete `init_scheduler` + the
`SCHEDULER` TLS and re-baseline the 4 `vm_async` RED GREEN, THREE blockers remain:

1. **Native agent-loop cooperative re-entry (Task 04, widened).** `run_tool_loop`
   / `__agent-drive` must drive provider/tool rounds cooperatively under the
   runtime so agents overlap and cancellation interrupts the loop
   (`agent_async_test` is 7/0 on legacy, 3/4 under the flip).
2. **Runtime `AwaitIo` support** for `event/select` / `io/read-key-timeout`
   cooperative yielding (family-B `event_select`).
3. **Injectable/virtual `RuntimeClock`** so `set_blocking_sleep_callback` advances
   logical time deterministically (family-B `blocking_sleep_hook`).

Once (1)–(3) land: route `run_exprs_on_vm` through `run_exprs_via_runtime`, delete
`init_scheduler`/`SCHEDULER` TLS, and re-baseline the 4 `vm_async_test` RED GREEN.
