# Task 06: Task Context, LLM, Agent, Workflow, MCP, and Tracing Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make each task’s explicit `TaskContext` the source of truth for dynamic
state, then migrate LLM/agent, workflow, MCP, and OpenTelemetry orchestration to
unified runtime suspension, ownership, cancellation, and accounting.

**Architecture:** Core context fields are copied/shared/reset according to the
master inheritance table. Subsystems store traced `TaskLocalValue` extensions.
The runtime installs a panic-safe guard for the active task during one VM/native
step only; external libraries may read a compatibility thread-local during that
guard, but capture/install callbacks and ambient TLS never own canonical state.
Higher-level fan-out is built on Task 04 owned scopes and Task 05 I/O jobs.

**Tech Stack:** Rust, `sema-core`, `sema-llm` FakeProvider, `sema-workflow`,
`sema-mcp`, `sema-otel`, structured async tests.

## Execution contract

- **Status:** Ready only after Task 05 is accepted and committed.
- **Dependencies:** Task 02 task-context extension shell and handle, final owned
  scopes, Task 05 jobs/resource cleanup, FakeProvider and existing subsystem
  fixtures. Task 06 defines and implements the guard and named-field migration.
- **Immutable inputs:** Master field-by-field inheritance table, sandbox
  non-widening, accounting/cache invariants, orchestration ownership, and trace
  lineage.
- **Exact start state:** Clean worktree; latest commit subject is
  `refactor(runtime): migrate interruptible resources`; Task 01–05 gates are
  GREEN, and the Task 01 inventory assigns every known task-local match to Task
  06 or a reviewed synchronous owner. Task 1 creates and reviews the exhaustive
  context/TLS matrix before production edits.
- **Parallel work:** After core context/guard tests merge, LLM, OTel, workflow,
  and MCP migrations may proceed in their own crates. One owner controls core
  task context, VM guard installation, stdlib/prelude integration, matrix, and
  guards. Agent orchestration starts after LLM primitives; integrated review
  starts after all subsystems merge.

## Global constraints

- Tasks 01–05 must be accepted. Resource operations use Task 05 jobs; language
  orchestration uses Task 04 ownership primitives.
- Every task-local TLS/callback match is classified. Process caches, interning,
  GC registries, and external library guards may remain only with a written
  non-task-local rationale.
- A child never inherits whichever task happened to run most recently.
- Sandbox/capabilities may stay equal or narrow; inheritance never widens them.
- Usage/budget aggregate objects are shared, while last-usage/retry/stream
  cursors are private snapshots or reset fields.
- LLM/agent tests use `FakeProvider`; no required gate consumes a live API key.
- A cache hit reports zero provider usage and zero new budget charge.
- Cancellation of an agent/workflow/MCP operation interrupts or boundedly
  quarantines resource work and reaps owned child tasks.
- No profiling or benchmarking in this layer.

---

## Files and responsibilities

**Create**

- `crates/sema-llm/src/task_context.rs` — LLM configuration/accounting/cursors.
- `crates/sema-otel/src/task_context.rs` — explicit span lineage and task state.
- `crates/sema-workflow/src/task_context.rs` — shared run handle/private guards.
- `crates/sema-mcp/src/task_context.rs` — shared handles/private request state.
- `crates/sema/tests/task_context_async_test.rs` — field-by-field suspension and
  sibling/root leakage tests.
- `crates/sema/tests/llm_runtime_test.rs` — deterministic FakeProvider runtime
  matrix.
- `crates/sema/tests/orchestration_runtime_test.rs` — agent/workflow/MCP owned
  cancellation and cleanup matrix.
- `docs/plans/evidence/unified-cooperative-runtime/task-06-context-matrix.md` —
  every context field and TLS/callback disposition.
- `docs/plans/evidence/unified-cooperative-runtime/task-06.md` — verification
  evidence.
- `docs/plans/reviews/unified-cooperative-runtime/task-06.md` — independent
  review report.

**Modify**

- `crates/sema-core/src/context.rs`, `runtime/task_context.rs`, `output_hook.rs`,
  `sandbox.rs`, `mcp_cassette.rs`, and `async_signal.rs` — remove canonical
  task-local callback/TLS stores.
- `crates/sema-vm/src/runtime/{task.rs,drive.rs}` — install/restore context guard
  on every task entry/exit, including panic, debug stop, and cancellation.
- `crates/sema-stdlib/src/{context,otel,workflow,workflow_mcp}.rs` and
  `crates/sema-eval/src/prelude.rs` — consume explicit context and owned scopes.
- `crates/sema-llm/src/{builtins,provider,embeddings,http,anthropic,openai,gemini,ollama,pricing,cassette}.rs` — explicit state, Task 05 jobs, cancellation.
- `crates/sema-otel/src/{lib,imp,compat,noop,testing}.rs` — explicit task spans
  and temporary external-library guard.
- `crates/sema-workflow/src/{lib,context,event,journal}.rs` — explicit run state
  and owned fan-out.
- `crates/sema-mcp/src/{lib,builtins,client,server,tools}.rs` — task state,
  cancellable calls, owned handlers.
- Existing LLM, agent, workflow, MCP, OTel, GC, and cassette tests named below.
- Runtime inventory, conformance guard, and legacy baseline.

Task 02 added only `HashMap<TypeId, Rc<dyn TaskLocalValue>>`,
`TaskContextHandle`, child-extension inheritance, and the optional handle on
`EvalContext`. This task owns the exhaustive field-by-field ownership table and
migration of existing sandbox, module/current-file, call-stack, output, tracing,
usage, and context fields; it must not assume those fields were already copied
into `TaskContext`.

## Exact context extension contracts

```rust
pub struct LlmTaskState {
    pub config: Rc<LlmConfigSnapshot>,
    pub budget: Option<Rc<BudgetAccount>>,
    pub usage: Option<Rc<UsageAccount>>,
    pub last_usage: Option<Usage>,
    pub retry_cursor: RetryCursor,
    pub stream_cursor: StreamCursor,
}

pub struct OtelTaskState {
    pub parent: SpanContext,
    pub stack: Vec<SpanHandle>,
    pub conversation: ConversationIds,
}

pub struct WorkflowTaskState {
    pub run: Rc<WorkflowRun>,
    pub active_step: Option<StepGuard>,
    pub request_state: WorkflowRequestState,
}

pub struct McpTaskState {
    pub handles: Rc<McpHandleRegistry>,
    pub cassette: Option<Rc<McpCassette>>,
    pub active_request: Option<RequestId>,
}
```

Inheritance is exact:

- LLM config is shared immutable; budget and usage accounts are shared;
  `last_usage`, retry cursor, and stream cursor reset for a child.
- OTel conversation IDs propagate, the current span becomes the child parent,
  and the child receives a new empty span stack.
- Workflow run handle is shared; active step guard and request state reset.
- MCP handle registry/cassette are shared; active request resets.

Every extension implements tracing and explicit `inherit()`. It must not put a
traceable `Value` behind an untraced `Any` or opaque closure.

`TaskContextGuard::enter(&TaskContext)` returns a guard whose `Drop` restores the
previous external-library state. Nested entry is stack-disciplined. A panic
test proves restoration. The runtime task record remains canonical before,
during, and after guard installation.

## Task 1: Produce the complete context/TLS matrix

**Files:** context matrix and inventory

- [ ] **Step 1: Discover ambient state**

```bash
rg -n 'thread_local!|set_.*callback|capture_.*scope|install_.*scope|current_.*scope|with_.*context|user_context|hidden_context|context_stacks|current_file|module_cache|output_hook|Sandbox' \
  crates/sema-core crates/sema-vm crates/sema-eval crates/sema-stdlib \
  crates/sema-llm crates/sema-otel crates/sema-workflow crates/sema-mcp
```

- [ ] **Step 2: Add one matrix row per match and context field**

Columns are `symbol`, `path`, `task-local?`, `canonical owner`, `child policy`,
`suspension test`, `replacement/deletion task`, and `remaining TLS rationale`.
No module-wide aggregate rows.

- [ ] **Step 3: Review the matrix before migration**

Reject “copy context” without a field policy and any shared mutable cursor that
should be private.

## Task 2: Implement context extensions and guard behavior

**Files:** four task-context modules, core task context, VM drive,
`task_context_async_test.rs`

- [ ] **Step 1: Write failing field-by-field tests**

For every master-table field: set a distinctive parent value, spawn two
siblings, mutate/suspend/resume them in alternating order, and assert the exact
share/snapshot/reset policy. Repeat with two roots using different output,
sandbox, current file, tracing parent, and host metadata.

- [ ] **Step 2: Add panic/debug/cancellation guard tests**

Panic inside a native step, stop at a breakpoint, cancel while suspended, and
drop the interpreter. After each path, the prior host guard state is restored
and no sibling observes the interrupted task’s private state.

- [ ] **Step 3: Implement extensions, tracing, and guard**

Use typed accessors; a missing extension is initialized from explicit root
options, never from whatever ambient TLS is active.

- [ ] **Step 4: Run**

```bash
cargo test -p sema-lang --test task_context_async_test
cargo test -p sema-core cycle
cargo test -p sema-lang --test gc_otel_test
```

Expected: field matrix, restoration, and cycle tests pass.

## Task 3: Migrate LLM calls, streaming, cache, usage, and budgets

**Files:** `sema-llm` files, `llm_runtime_test.rs`, existing LLM tests

- [ ] **Step 1: Add FakeProvider failure/suspension scripts**

Cover returned reply, provider error, network retry, retry cancellation, cache
hit, cache miss, streaming chunks with mid-stream cancellation, embeddings,
rerank/batch, fallback, concurrent children sharing one budget, independent
roots with different configuration, and late provider completion.

- [ ] **Step 2: Assert accounting invariants before implementation**

Exact assertions include request count, message/tool correlation, chunk order,
usage per task, aggregate budget charge, zero charge on cache hit, and no charge
from discarded late completion.

- [ ] **Step 3: Return `NativeOutcome` and submit Task 05 jobs**

Provider configuration comes from `LlmTaskState`. Decoders run on the runtime
thread. Retry timers use runtime timers. Remove behavior branches based on
`in_async_context` and task-local capture/install callbacks.

- [ ] **Step 4: Run deterministic targets**

```bash
cargo test -p sema-lang --test llm_runtime_test
cargo test -p sema-lang --test llm_fake_test
cargo test -p sema-lang --test llm_simple_async_test
cargo test -p sema-lang --test llm_chat_tools_async_test
cargo test -p sema-lang --test batch_rerank_async_test
cargo test -p sema-lang --test embed_async_otel_test
cargo test -p sema-lang --test embedding_api_test
cargo test -p sema-lang --test llm_cassette_test
```

Expected: all pass without provider keys or real sleeps.

## Task 4: Migrate agent loops and higher-level concurrency

**Files:** `sema-llm/src/builtins.rs`, prelude, agent tests,
`orchestration_runtime_test.rs`

- [ ] **Step 1: Write owned-agent tests**

Cover a tool call that suspends, parallel tools with one failure, parent
cancellation during provider call/tool call/backoff, breaker state, maximum turn
limit, stream cancellation, and concurrent agents sharing one budget but
private conversation cursors.

- [ ] **Step 2: Build fan-out on owned scopes**

Parallel tool/agent work cancels and reaps owned siblings on fail-fast paths.
Settled variants collect outcomes without sibling cancellation. Preserve tool
result IDs and assistant tool-call turns exactly.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test agent_async_test
cargo test -p sema-lang --test agent_async_breaker_test
cargo test -p sema-lang --test otel_agent_test
cargo test -p sema-lang --test orchestration_runtime_test -- agent
```

Expected: all pass and runtime snapshots show no owned child/provider job leak.

## Task 5: Migrate workflows and MCP

**Files:** workflow/MCP crates and stdlib modules, orchestration tests

- [ ] **Step 1: Add workflow/MCP ownership tests**

Cover workflow parallel success/failure/settled, budget sharing, journal order,
resume, cancellation during a leaf, MCP call cancellation, disconnect,
interactive request cancellation, handler shutdown, shared handles across
children, and private active request IDs.

- [ ] **Step 2: Implement explicit handles and scopes**

Workflow runs and MCP connections are shared resources in context; individual
step/request guards are private. Server handler fan-out is an owned scope. MCP
network calls use Task 05 interrupt hooks and generation-tagged completion.

- [ ] **Step 3: Run deterministic targets**

```bash
cargo test -p sema-workflow
cargo test -p sema-mcp
cargo test -p sema-lang --test workflow_budget_test
cargo test -p sema-lang --test workflow_cookbook_test
cargo test -p sema-lang --test workflow_resume_test
cargo test -p sema-lang --test workflow_tools_test
cargo test -p sema-lang --test workflow_mcp_seam_test
cargo test -p sema-lang --test workflow_mcp_e2e_test
cargo test -p sema-lang --test mcp_async_test
cargo test -p sema-lang --test mcp_builtin_test
cargo test -p sema-lang --test mcp_cassette_test
cargo test -p sema-lang --test orchestration_runtime_test -- workflow_mcp
```

Expected: all pass with local/fake transports and no dangling run/handler/job.

## Task 6: Migrate OpenTelemetry lineage and leakage behavior

**Files:** `sema-otel`, stdlib OTel module, OTel tests

- [ ] **Step 1: Add exact interleaving trace tests**

Two siblings and two roots suspend mid-span and resume in a forced alternating
order. Assert parent/child IDs, stack balance, tags, errors, cancellation links,
conversation grouping, no cross-root parentage, and no span left active after
panic/cancel/drop.

- [ ] **Step 2: Implement task-owned tracing state**

The compatibility TLS is installed only while calling the external OTel API.
Remove capture/install callbacks as state persistence mechanisms.

- [ ] **Step 3: Run all OTel targets**

```bash
cargo test -p sema-otel
cargo test -p sema-lang --test otel_ids_test
cargo test -p sema-lang --test otel_native_test
cargo test -p sema-lang --test otel_host_nesting_test
cargo test -p sema-lang --test otel_llm_test
cargo test -p sema-lang --test otel_embed_test
cargo test -p sema-lang --test otel_tags_test
cargo test -p sema-lang --test otel_error_test
cargo test -p sema-lang --test otel_cassette_test
```

Expected: all pass; span stacks are empty after every test scenario.

## Task 7: Static guards, full verification, and independent review

- [ ] **Step 1: Strengthen context conformance checks**

Fail on removed task capture/install callback symbols, direct task-local TLS
reads outside approved guard modules, `in_async_context` behavior branches, and
higher-level fan-out implemented as observational `async/all` plus detached
spawn.

- [ ] **Step 2: Run layer gates**

```bash
cargo test -p sema-core -p sema-vm -p sema-llm -p sema-otel \
  -p sema-workflow -p sema-mcp
cargo test -p sema-lang --test task_context_async_test
cargo test -p sema-lang --test llm_runtime_test
cargo test -p sema-lang --test orchestration_runtime_test
cargo test -p sema-lang --test runtime_conformance_test
cargo fmt --all -- --check
cargo clippy -p sema-core -p sema-vm -p sema-llm -p sema-otel \
  -p sema-workflow -p sema-mcp --all-targets -- -D warnings
scripts/check-unified-runtime-legacy.sh > /tmp/runtime-legacy.actual
diff -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline /tmp/runtime-legacy.actual
git diff --check
```

Expected: all GREEN and every remaining TLS match has a reviewed matrix row.

- [ ] **Step 3: Assign independent context/orchestration review**

Finding IDs use `UR-T06-R###`. Reviewer follows one field from root creation
through child inheritance and alternating suspension, audits all four extension
tracers, verifies accounting with FakeProvider records, and injects cancellation
at each agent/workflow/MCP await point.

- [ ] **Step 4: Fix findings test-first and rerun the complete target list**

Any leakage found in one field requires adding the same interleaving shape to
the generic context matrix, not only a subsystem-specific regression.

- [ ] **Step 5: Commit the accepted layer**

```bash
git add crates/sema-core crates/sema-vm crates/sema-eval crates/sema-stdlib \
  crates/sema-llm crates/sema-otel crates/sema-workflow crates/sema-mcp \
  crates/sema/tests docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  docs/plans/reviews/unified-cooperative-runtime
git commit -m "refactor(runtime): make task context explicit"
```

## Completion criteria

- Every task-local field and TLS/callback is classified and tested.
- Context inheritance matches the master table field by field across suspension.
- Panic, debugger, cancellation, and shutdown restore external guards.
- LLM, streaming, retry, cache, usage, and budget behavior is deterministic and
  FakeProvider-covered.
- Agent/workflow/MCP fail-fast paths use owned scopes and reap children/jobs.
- Tracing lineage remains correct under forced sibling/root interleaving.
- Static guards reject ambient task-state ownership and old async branching.
- Independent review and durable evidence are clean.
