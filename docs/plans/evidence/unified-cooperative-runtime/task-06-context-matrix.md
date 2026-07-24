# Task 06 — Context / TLS matrix

**Phase:** P4 (TaskContext generalization). **Author:** implementation pass, 2026-07-16.

Purpose: classify every thread-local / ambient-context store reachable from a
runtime task, and record its disposition under the unified cooperative runtime:

- **swapped** — per-task *dynamic* state that two interleaving tasks would corrupt
  if they shared one thread-local. Captured at `async/spawn` and swapped into/out of
  the process thread-locals around each quantum by the single panic-safe
  `TaskScopeSwap` guard (`crates/sema-vm/src/runtime/state.rs`), driven from the
  `TASK_SCOPE_SEAMS` table.
- **stays (threaded)** — context that is passed explicitly (as a `Value` argument, a
  handle key, or through the parked native continuation) rather than read from an
  ambient thread-local, so interleaving cannot cross it.
- **stays (process)** — process/evaluator-level infrastructure: interned tables, GC
  collector state, resource registries keyed by handle, caches, once-flags, host
  routing hooks, and test seams. Not per-task; sharing is correct.

## Swapped per quantum (the `TASK_SCOPE_SEAMS` table)

| # | Seam (sema-core) | Backing TLS (owner) | Child inherit policy |
|---|---|---|---|
| 0 | `current_llm_scope_boxed` / `take_task_llm_scope` / `install_task_llm_scope` | `LlmDynScope` — `CACHE_ENABLED`, `CALL_TAGS`, active budget frame `Rc` (`sema-llm/src/builtins.rs:248`) | config/cache flags snapshot-copied; budget frame shared `Rc` (one aggregate charge across a fan-out) |
| 1 | `current_conversation_scope_boxed` / `take_task_otel` / `install_task_otel` | `OtelTaskCtx` — span stack + conversation/session/user ids (`sema-otel/src/imp.rs:83`) | conversation ids propagate; child gets an EMPTY span stack (parents to its own trace root) |
| 2 | `current_usage_scope_boxed` / `take_task_usage_scope` / `install_task_usage_scope` | `UsageScope` — leaf-usage accumulator `Rc` + `LAST_USAGE` (`sema-llm/src/builtins.rs:164`) | leaf-usage scope shared with the spawning `workflow/step`; per-leaf attribution stays with the step that spawned it |

These three were the live otel/usage corruption bug (the P-hotfix): the legacy
scheduler swapped all three; the unified runtime originally swapped only the LLM
scope. P4 generalizes the swap into one guard + a table so all three (and any
future entry) are driven uniformly. **Behaviour is identical to the prior
three-field swap** — the state.rs `scope_swap_tests` unit tests and the
`task_scope_isolation_test` / `otel_agent_test` / `embed_async_otel_test` suites
stay green.

## Investigated for a latent per-task swap bug — workflow & MCP: NOT swapped

The hotfix covered exactly the scopes the legacy scheduler swapped. P4's brief was
to check whether **workflow** and **MCP** hold per-task *dynamic* thread-local state
of the same class (each task owns its own context, corrupted by sharing one TLS). They
do not — both thread context explicitly — so no swap seam was added for them.

### MCP — threads context explicitly (no per-task dynamic TLS)

`crates/sema-mcp/src/builtins.rs:79` declares the only MCP thread-locals:

- `CONNECTIONS: HashMap<String, Rc<ConnEntry>>` — a **registry keyed by the handle
  string**. A task acts on a connection only via the handle `Value` it is given; the
  per-request checkout state (`Slot::Available`/`CheckedOut`/`Tombstone`) lives on the
  `ConnEntry` in this registry, not in an ambient per-task slot. Two tasks holding two
  different handles touch two different entries; a single connection's own calls queue
  via the checkout, exactly as the serial JSON-RPC pipe requires. This is
  process/evaluator infrastructure keyed by an explicit handle → **stays (process)**.
- `SANDBOX: Sandbox` — captured **at `register_mcp_builtins` time** (per evaluator),
  consulted to gate the OAuth browser launch. Not per-task; the offload path even
  resolves the decision on the VM thread and bakes it in as a `bool` precisely because
  the background thread's `SANDBOX` is unpopulated (`builtins.rs:146`). → **stays (process)**.

There is no MCP "active request id" thread-local. The `McpTaskState` shape sketched in
the Task-06 plan (shared handle registry / private active request) is already realised
structurally: the shared registry is `CONNECTIONS`, and the "active request" is the
in-flight checkout on the `ConnEntry`, correlated by the runtime's own
generation-tagged external completion — not an ambient TLS. **No swap needed.**

### Workflow — run context rides the parked continuation; run handle shared by design

`crates/sema-workflow/src/context.rs:39` declares `WORKFLOW: Option<Rc<WorkflowCtx>>`,
installed by a run-scoped panic-safe RAII guard (`WorkflowGuard`, `install_scope`).
Under the runtime, `workflow/run` is a dual-ABI thunk native (`register_thunk_fn`,
`crates/sema-stdlib/src/workflow.rs:228`): its pre-thunk `plan` opens the scope and
moves the `WorkflowGuard` into the teardown state, which is moved into the suspended
`ThunkContinuation`. So the guard — and therefore the installed `WORKFLOW` context —
**rides the parked native continuation for the whole run**; it is threaded through the
suspended stack, not captured-from-TLS at spawn like otel/usage.

Two isolation questions:

1. **Fan-out leaves within one run.** A `parallel`/`pipeline` fan-out spawns leaf
   tasks that all belong to the **same run**, so they intentionally share the one
   `Rc<WorkflowCtx>` (shared run handle, journal, budget). Sharing is correct, not
   corruption — the analogue of the otel bug would be *different* contexts colliding,
   which cannot happen here. The genuinely per-leaf state that MUST isolate — LLM
   **usage attribution** — already rides the swapped usage-scope seam (row 2 above),
   captured at spawn from the `workflow/step` that launched the leaf. The remaining
   private field `cur_agent_id` (`context.rs:88`) is clobbered under true concurrent
   interleaving, but this is a **pre-existing, explicitly documented best-effort**
   limitation (see the `cost_spent`/`WorkflowCtx` comments at `context.rs:66-68,375`),
   affecting only `tool-call`→agent attribution *labeling* — not run-handle isolation,
   journal integrity, or budget correctness (the budget rides the shared `Rc`). It is
   not a regression introduced by the runtime migration, and fixing it would require
   moving the run/step handle into the spawn-captured task context — a behaviour change
   out of this behaviour-preserving refactor's scope.
2. **Two concurrent runs.** Running two `workflow/run`s as sibling spawned tasks would
   let the second run's `install_scope` displace the first's `WORKFLOW` context. This is
   not a supported pattern (a run owns a run directory + journal; concurrent runs in one
   process/thread are not an intended use), there is no spawn-capture wiring for it, and
   no test or example exercises it. Documented here as the one theoretical residue; if a
   supported use ever arises, the fix is to add `WorkflowCtx`/`McpTaskState` as
   `TASK_SCOPE_SEAMS` entries (one table row each) exactly like the three above.

**Conclusion:** neither workflow nor MCP holds per-task dynamic TLS requiring a
per-quantum swap. No failing interleave test was landed because there is no bug of the
otel/usage class to drive — MCP keys by explicit handle, workflow shares its run handle
by design and rides the parked continuation. Skipped per the plan's "note it and skip"
branch.

## Stays (process / evaluator infrastructure)

Every remaining thread-local reachable from a task, with why it is not per-task dynamic:

| TLS | Location | Rationale |
|---|---|---|
| `STDLIB_CTX` | `sema-core/src/context.rs:815` | shared stdlib `EvalContext` for callback dispatch; process-level |
| `STDOUT_HOOK` / `STDERR_HOOK` | `sema-core/src/output_hook.rs:8` | host output-capture routing hook; set by host, root-tagging is a P6 concern |
| `HOOK` (mcp cassette) | `sema-core/src/mcp_cassette.rs:27` | record/replay seam registered by host; not per-task |
| async-signal callbacks + scope registrations | `sema-core/src/async_signal.rs:27,98,119,147,204,266,332,382` | the seam *registration* cells (fn-pointer installs) + current-task-id/resume/yield plumbing; the three scope seams themselves are the swapped ones above |
| Spur interner | `sema-vm/src/lower.rs:13,298,364`, `sema-core/src/value.rs:28,94,2584` | interned symbol/keyword tables; process-global by definition |
| GC collector state | `sema-core/src/cycle.rs:525` | cycle-collector working set; process-level |
| payload-tracer once-guard | `sema-vm/src/vm.rs:215` | per-thread registration once-guard |
| native-call VM stack | `sema-vm/src/vm.rs:589` | re-entrancy stack for HOF callbacks; unwound synchronously within a call |
| `ACTIVE_DEBUG` DebugState stack | `sema-vm/src/vm.rs:814`, `sema-eval/src/debug_session.rs:16` | host-owned debug session, reached via TLS by design (P3); not task-owned |
| `PROVIDER_REGISTRY`, `LISP_PROVIDERS` | `sema-llm/src/builtins.rs:26,348` | LLM provider registries; process-level |
| `SESSION_USAGE` | `sema-llm/src/builtins.rs:26` | global session cost accumulator, deliberately independent of any budget scope (`builtins.rs:833`) |
| `PRICING_WARNING_SHOWN`, `CACHE_ENABLED` default, retry base-ms | `sema-llm/src/builtins.rs:348,8059` | once-flags + test seams (`set_retry_base_ms`) |
| `AGENT_RUNS`, `STREAM_RUNS` | `sema-llm/src/builtins.rs:8940,10084` | live non-blocking run registries keyed by an explicit integer token handed to Sema |
| pricing cache | `sema-llm/src/pricing.rs:92` | price table cache; process-level |
| DROP_TEMPERATURE / compat learned sets | `sema-llm/src/openai.rs:12` | per-model 400-learned compat cache; process-level |
| OTel imp exporter/provider | `sema-otel/src/imp.rs:83` | the OTel context TLS is swapped (row 1); the exporter/provider handles are process-level |
| resource registries | `sema-stdlib/src/{proc,sqlite,kv,serial,pty,system,terminal,fs_watch,io}.rs` | handle-keyed checkout registries + stdin/tty state; threaded by explicit handle |
| regex cache | `sema-stdlib/src/string.rs:140` | compiled-regex cache; process-level |
| VFS backend | `sema-core/src/io_backend.rs:92` | virtual filesystem backend; process/host-level |
| workflow MCP resolver | `sema-stdlib/src/workflow_mcp.rs:473` | process' registered resolver install; not per-task |

No entry in this table holds per-task dynamic state that two interleaving tasks would
corrupt; each is either explicitly threaded or genuinely process-scoped.
