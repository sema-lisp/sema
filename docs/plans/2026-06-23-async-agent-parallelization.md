# Concurrent `agent/run` on the Cooperative Scheduler

**Status:** design + spike plan (not started) — **reviewed 2026-06-23, see §8 Verification addendum: 5 blockers must be resolved before Phase 2**
**Date:** 2026-06-23
**Owner:** repo owner (build target)
**Scope (v1):** make `agent/run` completions overlap when several agents are spawned as scheduler tasks. `llm/stream`, `llm/pmap`, and `batch_complete` keep their existing paths.

---

## 1. Problem & why it matters

Sema has a real cooperative scheduler. `async/spawn` gives each task its own VM (`scheduler.rs`), and `run_until_reentrant` (`scheduler.rs:497`) round-robins ready tasks, parking them on `YieldReason::{AwaitPromise, ChannelRecv, ChannelSend, Sleep}` (`async_signal.rs:19`). Channels and sleeps already overlap across tasks. The async feature *looks* done.

It is not done for the one workload people actually want to parallelize: agents. The reason is a single line. `agent/run` (`builtins.rs:~2290`) drives `run_tool_loop` (`builtins.rs:~5441`), whose per-round provider call ends at `provider.complete(req)`:

```rust
// anthropic.rs:~429
fn complete(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
    self.runtime.block_on(self.complete_async(req))
}
```

`block_on` parks **the one VM thread** for the entire HTTP round-trip. So when a user writes the obvious thing:

```sema
(async/all
  (map (fn (q) (async/spawn (fn () (agent/run a q))))
       '("q1" "q2" "q3" "q4")))
```

each spawned task takes its scheduler turn, reaches `block_on`, and freezes the VM thread until *its* HTTP call returns. Task B never starts its request until task A's round fully completes. Four 800 ms agents take ~3200 ms wall, not ~800 ms. **The scheduler's whole point — overlap latency — is defeated at the exact boundary where latency lives.** Until this is fixed, `async/spawn` + `agent/run` is a lie: it spawns tasks that cannot actually run concurrently.

The fix is to make the LLM round-trip a fourth *yield source*, so a task awaiting HTTP parks (like it does for a channel) and the VM thread runs the siblings, who each launch their own request. N requests then fly simultaneously on a background runtime; wall-clock approaches `max(latency_i)` instead of `sum(latency_i)`.

---

## 2. Chosen architecture

**Thread-offload `AwaitIo` yield, with the agent round-loop lifted out of the native `run_tool_loop` frame into a Sema-level prelude loop.** Two ideas, merged:

1. **From Candidate A — the yield + Send boundary.** A new leaf native `llm/chat-once` builds the fully-resolved `ChatRequest` on the VM thread, spawns the wire call on a shared multi-thread tokio runtime, and yields `YieldReason::AwaitIo(Rc<IoHandle>)`. The scheduler parks the task and runs siblings; when the response lands it resumes the task with the `ChatResponse`-as-`Value`.
2. **From Candidate B — runtime ownership.** Store providers as `Arc<dyn LlmProvider>` and spawn the provider's **own** `complete_async` on one shared runtime. This kills A's biggest risk (re-implementing send/parse/retry in a hand-rolled `IoJob`, which would drift from `complete_async` and diverge from the FakeProvider tests).

> ⚠️ **BLOCKER B1 (see §8).** `complete_async` is **not on the `LlmProvider` trait** — it is an *inherent* method on each concrete struct (`impl AnthropicProvider`, `anthropic.rs:13/129`); the trait (`provider.rs:6`) exposes only the **sync** `complete()`, which does `self.runtime.block_on(...)` on the provider's *own* runtime. Through `Arc<dyn LlmProvider>` you can only reach `complete()`, and spawning that onto `SHARED_RT`'s async pool **panics** ("Cannot start a runtime from within a runtime", `http.rs:8-17`). This entire "spawn the provider's own `complete_async`, no re-implementation" premise must be resolved (add an async trait method across all 4 providers **and** FakeProvider, **or** use `spawn_blocking(|| provider.complete(req))`) before Phase 2.

### Why the round-loop *must* leave the native frame

This is the non-negotiable constraint that rejects the "just suspend `run_tool_loop`" variants of B and C. The VM's resume model is bytecode-level, not Rust-frame-level:

- On `AsyncYield`, the VM saves `frame.pc`, pushes a nil placeholder, and returns (`vm.rs:~1480`).
- On resume, the scheduler does `task.vm.replace_stack_top(resume_value)` (`scheduler.rs:627`) and re-enters `run_async`, which **continues the bytecode loop at the instruction after the `CALL`**.

The yielding native function's Rust call frame is **gone**. A native fn that yields is necessarily a *leaf* whose entire post-resume effect is "the `CALL` evaluated to the resume value." But `run_tool_loop` (`builtins.rs:~5484`) is a monolithic synchronous native frame: its `for _round` loop holds `messages`, `consecutive_errors`, the round counter, **and** non-Send `Rc` otel guards (`_agent_span`, `_conv_scope`, `_tele`) live across the `do_complete` call sitting in the middle of the loop. You physically cannot suspend that frame across an HTTP wait and resume back into the middle of a Rust `for`.

So the round loop is rewritten as a **Sema prelude function** that threads `messages` / `consecutive-errors` / `round` as ordinary `Value`s and calls a **leaf** `llm/chat-once` as the *sole* yield point, dispatching tools as ordinary Sema between calls. The existing `replace_stack_top` + `take_resume_value` machinery carries the `ChatResponse` back exactly as it already does for `await`/channel resume.

### The Send boundary, made explicit

The hard constraints (`Value`/`Env` are `Rc`, not `Send`; `PROVIDER_REGISTRY` is `thread_local`; resume re-enters the VM at the yield point) are all respected because **only two known-Send structs traverse the thread boundary**.

**Crosses to a `SHARED_RT` worker thread:**
- the fully-resolved `ChatRequest` (plain Send struct — built from `Value`s *on the VM thread* before the yield),
- an `Arc<dyn LlmProvider>` cloned out of the `thread_local` `PROVIDER_REGISTRY` (`builtins.rs:~27`) under `.with` on the VM thread (trait is already `Send + Sync`, `provider.rs:6`) — so the worker calls the provider's **own** `complete_async`, no re-implementation **(⚠️ blocked: `complete_async` is not trait-reachable — see §8 B1)**,
- `max_retries: u32` and the `oneshot::Sender<Result<ChatResponse, LlmError>>`.

**Returns across the boundary:** `Result<ChatResponse, LlmError>` (Send).

**Never leaves the VM thread:** every `Value` and `Env`; the `Rc<IoHandle>` embedded in the `YieldReason` and the `oneshot::Receiver` inside it; the entire otel span stack (`_agent_span` / `_conv_scope` — these are thread_local-stack-bound `opentelemetry::Context` guards, **not** `Rc`; **⚠️ staying on the VM thread is precisely why they corrupt under task interleaving — see §8 B2**); all `thread_local` accounting (`track_usage`, `BUDGET_*`, `CACHE_*`, `SESSION_USAGE`, `LAST_SERVING_PROVIDER`); cache/cassette lookup-and-store; and the `ChatResponse` → `Value` decode. The `PROVIDER_REGISTRY` itself is never moved — only an `Arc` clone is taken under `.with`, and the borrow is released before `spawn`. Both request construction and response decoding run on the VM thread, so the `Rc` graph is untouched — only wire bytes and the two Send structs traverse the boundary.

To keep `sema-core` off both `sema-llm` **and** tokio, `IoHandle` carries a boxed poller closure (created in `sema-llm`) that owns the `oneshot::Receiver` and does the `ChatResponse` → `Value` decode on the VM thread. `sema-core` never names a `sema-llm` type.

---

## 3. Exact code changes (ordered)

Each step cites `file:symbol`. Steps 1–5 are the scheduler/yield plumbing (Phase 1, de-risked by the spike in §5). Steps 6–8 are the agent loop. Steps 9–10 are cancellation + tests.

### Step 1 — shared runtime
`crates/sema-llm/src/http.rs`: add a process-wide multi-thread runtime.

```rust
#[cfg(not(target_arch = "wasm32"))]
static SHARED_RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub fn shared_rt() -> &'static tokio::runtime::Runtime {
    SHARED_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("SHARED_RT")
    })
}
```

This is the one runtime that drives all offloaded futures concurrently. Keep `BlockingRuntime` / `create_runtime` for the non-async top-level path (`llm/complete` outside a scheduler) so that path and the sync FakeProvider tests are untouched.

### Step 2 — `Arc`-ify the provider registry
`crates/sema-llm/src/provider.rs`: change `ProviderRegistry` to store `HashMap<String, Arc<dyn LlmProvider>>` and have `get` / `default_provider` return `Arc<dyn LlmProvider>` (clone the `Arc`). The trait is already `Send + Sync` (`provider.rs:6`). This lets the VM thread clone an `Arc` out of the `thread_local` registry under `.with`, release the borrow, and move the `Arc` into a spawned future — spawning the provider's own `complete_async`, not a copy.

### Step 3 — `AwaitIo` yield reason
`crates/sema-core/src/async_signal.rs:YieldReason` (line 19): add a variant. Keep `sema-core` free of tokio **and** `sema-llm`:

```rust
pub enum IoPoll {
    Pending,
    Ready(Result<Value, String>),
}

pub struct IoHandle {
    poll: RefCell<Box<dyn FnMut() -> IoPoll>>,
}
impl IoHandle {
    pub fn new(f: impl FnMut() -> IoPoll + 'static) -> Self {
        Self { poll: RefCell::new(Box::new(f)) }
    }
    pub fn poll(&self) -> IoPoll { (self.poll.borrow_mut())() }
}

pub enum YieldReason {
    AwaitPromise(Rc<AsyncPromise>),
    ChannelRecv(Rc<Channel>),
    ChannelSend(Rc<Channel>, Value),
    Sleep(u64),
    AwaitIo(Rc<IoHandle>),   // NEW
}
```

> ⚠️ **Note (§8 minor):** `YieldReason` derives `#[derive(Debug, Clone)]` (`async_signal.rs:18`). `IoHandle` holds `RefCell<Box<dyn FnMut() -> IoPoll>>`, which is neither `Debug` nor `Clone` — wrapping in `Rc` restores `Clone`, but a **manual `impl Debug for IoHandle`** is required or the derive on `YieldReason` won't compile.

The closure (built in `sema-llm`, step 6) owns the `oneshot::Receiver`, does `try_recv`, and on `Ok` does the `ChatResponse` → `Value` decode. tokio and `ChatResponse` never appear in `sema-core`.

### Step 4 — wake arm
`crates/sema-vm/src/scheduler.rs:wake_blocked_tasks` (line 149): add an `AwaitIo(h)` arm mirroring the existing `AwaitPromise` arm:

```rust
YieldReason::AwaitIo(h) => match h.poll() {
    IoPoll::Pending          => WakeAction::Pending,
    IoPoll::Ready(Ok(v))     => WakeAction::Resume(v),
    IoPoll::Ready(Err(msg))  => WakeAction::Fail(msg),
},
```

### Step 5 — park-on-IO in the all-blocked branch
`crates/sema-vm/src/scheduler.rs:run_until_reentrant` all-blocked branch (lines ~543–582). Today, with no Ready task and no Sleep-blocked task, it returns **"async scheduler: all tasks blocked (deadlock detected)"** (`scheduler.rs:582`). New behavior:

- If `>= 1` task is `Blocked(AwaitIo)`, do **not** declare deadlock and do **not** advance `virtual_now` for it.
- Park the VM thread on a shared completion signal (a `tokio::sync::Notify` or a std `Condvar` + counter that each spawned future bumps on completion), with the **same `check_interrupt()` cadence as `blocking_sleep_ms`**, then `continue` to re-run `wake_blocked_tasks`.
- Park **only** when zero Ready tasks **and** `>= 1` AwaitIo task, re-checked each pass, so any runnable VM/tool work pre-empts the park.

This is the **only** place the VM thread blocks, and it wakes on the first response. It sits in the all-blocked branch, *outside* a task step, so it doesn't disturb the `ReinstallGuard` dummy-swap inside the step.

### Step 6 — `llm/chat-once` leaf native
`crates/sema-llm/src/builtins.rs`: add `register_fn_ctx(env, "llm/chat-once", ...)`. Strict leaf semantics:

> ⚠️ **§8 correction (6a is dead code under `CALL_NATIVE`).** Resume does **not** re-invoke the native: on `AsyncYield` the VM saves `pc` *past* the `CALL_NATIVE` and on resume `replace_stack_top(resume_value)` deposits the response into the call's result slot, continuing the bytecode after the call (`vm.rs:1490-1497`, `scheduler.rs:627`). So `take_resume_value()` is `None` on resume and the guard below never fires (it mirrors the shipped `async/await` pattern at `async_ops.rs:117`, which is likewise vestigial — `set_resume_value` has **no caller**). Keep it only for symmetry; the actual response arrives as the `CALL`'s value.

```rust
// (a) resume path (vestigial under CALL_NATIVE — see note above):
if let Some(resp_val) = sema_core::take_resume_value() {
    return Ok(resp_val);
}

// (b) build the resolved ChatRequest from args (model/system/tools/messages
//     already substituted by the Sema loop) and do the SYNCHRONOUS
//     cache/cassette short-circuit on the VM thread. A cache HIT returns
//     immediately and NEVER yields — preserves the zero-usage invariant
//     (do_complete:~4790).
// ⚠️ §8 B5: `cache_lookup` is NOT a separable function today. The cache short-circuit
//    lives INSIDE do_complete (builtins.rs:4731-4822), tangled with span setup, the
//    conversation scope, cassette record/replay (run_completion), model-resolution for
//    the cache key (primary_model_for_cache, 4782-4788), and the fallback chain. A faithful
//    leaf must first extract a synchronous on-VM-thread cache/cassette/key-resolution stage.
if let Some(hit) = cache_lookup(&req) { return Ok(response_to_value(hit)); }

// (c) miss: clone Arc<provider> + max_retries out of the registry on-thread,
//     release the borrow, spawn the provider's own complete_async with retry
//     moved INTO the future.
let provider = PROVIDER_REGISTRY.with(|r| r.borrow().get(&name)); // Arc clone
let (tx, rx) = tokio::sync::oneshot::channel();
shared_rt().spawn(async move {
    let r = run_async_with_retry(provider, req2, max_retries).await; // tokio::time::sleep backoff
    let _ = tx.send(r);
});
let handle = Rc::new(IoHandle::new(move || match rx.try_recv() {
    Err(TryRecvError::Empty)  => IoPoll::Pending,
    Ok(Ok(resp))             => IoPoll::Ready(Ok(response_to_value(&resp))),
    Ok(Err(e))               => IoPoll::Ready(Err(e.to_string())),
    Err(Closed)              => IoPoll::Ready(Err("io worker dropped".into())),
}));
sema_core::set_yield_signal(YieldReason::AwaitIo(handle));
Ok(Value::nil())
```

Retry/backoff lives **inside** `run_async_with_retry` via `tokio::time::sleep`, so it never blocks the VM thread (this retires `complete_with_retry`'s `std::thread::sleep` at `builtins.rs:~5091` for the async path).

### Step 7 — Sema-level round loop
`crates/sema-eval/src/prelude.rs`: add `agent/-run-loop` as a Sema function (port `run_tool_loop`'s body). It threads `messages`, `consecutive-errors`, `round` as values, calls `(llm/chat-once ...)` as the **single** yield point per round, and dispatches tools as ordinary Sema between calls — preserving:

> ⚠️ **§8 B4: "thread `messages` as ordinary `Value`s" does not hold today.** The existing `Value`↔`ChatMessage` conversions (`sema_list_to_chat_messages` 5390 / `chat_messages_to_sema_list` 5423) carry only `:role`/`:content` and **drop** `tool_calls`/`tool_call_id`/`tool_name`. There is currently **no** Sema representation that round-trips `assistant_with_tool_calls` or `tool_result`, so the correlation invariant below cannot survive a Sema round-trip without **net-new full-fidelity marshaling natives**. ⚠️ **Also (§8 B6 hazard):** `llm/chat-once` must be called only from the loop body's direct call position — a yield inside an HOF callback frame (`map`/`filter`/`foldl`/a tool handler → `run_nested_closure`) is converted to an error, not resumed (`vm.rs:757-767`).
- the assistant `tool_calls` echo (`builtins.rs:~5518`),
- correlated `tool_result` ordering (`builtins.rs:~5592`),
- the consecutive-error bound and max-rounds cap.

otel / `track_usage` / budget stay as native helpers the Sema loop calls **on the VM thread after each resume** — they remain `thread_local`-correct because they never run across the yield.

### Step 8 — gate `agent/run`
`crates/sema-llm/src/builtins.rs:agent/run` (`builtins.rs:~2290`): when `sema_core::in_async_context()` **and** a scheduler is registered, delegate to `agent/-run-loop`; otherwise keep the synchronous `run_tool_loop` (top-level CLI / non-async path unchanged; FakeProvider sync tests unaffected). `llm/stream`, `llm/pmap`, `batch_complete` keep their existing paths.

### Step 9 — cancellation (best-effort)
`crates/sema-vm/src/scheduler.rs:cancel_task` (`scheduler.rs:242`) / `async/cancel` (`async_ops.rs:213`): on cancel, the in-flight tokio future runs to completion and its `oneshot::Receiver` is dropped (the spawned future tolerates a dropped `tx.send`). Document as a known limitation; an `AbortHandle` for true cancellation is a later add.

### Step 10 — tests
`crates/sema/tests/llm_fake_test.rs`: add a deterministic FakeProvider test exercising the `AwaitIo` path — a 2-round tool loop, a retry, a cache-hit-that-does-not-yield, and a budget assertion. This is the required CI regression oracle (CLAUDE.md). Add the §5 overlap benchmark as an `#[ignore]`d live test.

---

## 4. Multi-round tool-loop handling

`agent/run` is not one HTTP call — it is a loop where, between provider calls, the VM runs arbitrary tool handlers (`execute_tool_call` → `sema_core::call_callback`, `builtins.rs:~5650`). The design keeps tool execution exactly there: on the VM thread, synchronously, **between** yields. Only the per-round provider call (`llm/chat-once`) yields.

One agent's timeline:

```
[VM: build request]  ->  YIELD AwaitIo  ->  (scheduler runs OTHER tasks / parks on IO)
  ->  RESUME with ChatResponse  ->  [VM: run tools, accumulate messages]
  ->  [VM: build next request]  ->  YIELD AwaitIo  ->  ...  (until done / max rounds)
```

Key invariants:

- **Within one agent, rounds stay strictly ordered.** Round R+1's request is built only after round R's tools have run. Concurrency is *across* agents (each parked on its own round-R IO), never within one agent.
- **Tool-result correlation is preserved.** The Sema loop emits the assistant `tool_calls` turn and the correlated `tool_result` messages in the same order `run_tool_loop` does (`builtins.rs:5518`, `5592`); each provider serializer maps them to its native shape unchanged. Plain user-text results would silently break OpenAI-family providers — the Sema loop must not regress this.
- **Accounting stays on the VM thread.** `track_usage` / cache-store / `span.set_response` run on resume, on the VM thread. A cache hit short-circuits in `llm/chat-once` (step 6a) and never spawns, so it reports zero usage — the accounting invariant holds.
- **Interleaving is at yield boundaries only.** While task A runs a tool between rounds, task B can be parked on IO and vice-versa. Tool handlers that themselves do blocking IO (`http/get`) stay serial unless separately ported — the LLM round-trips, which dominate latency, are what overlap in v1.

---

## 5. Spike + acceptance oracle

### Throwaway spike — prove overlap before touching the agent loop

Wire up steps 3, 4, 5 (the `AwaitIo` yield + wake arm + park-on-IO branch) and **nothing in `run_tool_loop`**. Add one throwaway leaf native `llm/io-sleep-once` that mimics `llm/chat-once` but does a timer instead of an HTTP call:

```rust
// register_fn_ctx(env, "llm/io-sleep-once", ...)
if let Some(v) = sema_core::take_resume_value() { return Ok(v); }
let id = args[0].as_int().unwrap_or(0);
let (tx, rx) = tokio::sync::oneshot::channel();
shared_rt().spawn(async move {
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    let _ = tx.send(Value::int(id));
});
let handle = Rc::new(IoHandle::new(move || match rx.try_recv() {
    Err(TryRecvError::Empty) => IoPoll::Pending,
    Ok(v)                    => IoPoll::Ready(Ok(v)),
    Err(Closed)              => IoPoll::Ready(Err("dropped".into())),
}));
sema_core::set_yield_signal(YieldReason::AwaitIo(handle));
Ok(Value::nil())
```

Run from Sema:

```sema
(let ((t0 (sys/elapsed)))
  (async/all
    (map (fn (i) (async/spawn (fn () (llm/io-sleep-once i))))
         '(0 1 2 3 4)))
  (/ (- (sys/elapsed) t0) 1000000))  ; ms
```

**PASS:** five 1000 ms sleeps, spawned as five tasks, complete in **~1000 ms** wall (max), not ~5000 ms (sum). If the spike shows ~1 s, the hard part — overlap across the Send boundary via the per-task-VM scheduler — is proven, and only the Sema-loop lifting (steps 6–8) remains. Use `sys/elapsed` (ns), never `time/now-ms` (1 ms resolution), per MEMORY.

### Acceptance oracle — real agents overlap

With a real provider (cheap model, e.g. `claude-haiku-4-5`) or a Fake provider injecting a fixed ~800 ms latency per call:

```sema
(let ((a  (agent/new {:model "claude-haiku-4-5" }))
      (qs '("q1" "q2" "q3" "q4")))
  (let ((t0  (sys/elapsed))
        (res (async/all (map (fn (q) (async/spawn (fn () (agent/run a q)))) qs)))
        (ms  (/ (- (sys/elapsed) t0) 1000000)))
    (list (length res) ms)))
```

**PASS iff:**
1. `length res == 4` and each result is the model's answer — **correctness preserved**.
2. `ms` is within ~1.3× of the **slowest single** `agent/run` latency (≈ `max_i`), **not** the sum. For four single-round ~800 ms calls, `ms ≈ 800–1100 ms`, decisively below the ~3200 ms serial floor today's `block_on` (`anthropic.rs:~429`) produces.

**Second observability oracle:** instrument `SHARED_RT` spawn/complete with an `AtomicUsize` in-flight counter and assert peak `>= 2` (ideally `N`). This directly proves N requests are in flight *simultaneously*, not merely that the wall-clock was fast.

**Deterministic CI gate:** run the same shape under FakeProvider with `set_retry_base_ms(0)` (no real sleeps) and assert correctness + in-flight `>= 2` without timing flakiness.

---

## 6. Risks & honest caveats

- **Phase 3 is the real cost and the real risk.** Lifting `run_tool_loop`'s round loop into a Sema prelude loop while preserving the assistant-`tool_calls` echo, correlated `tool_result` ordering, and the budget/cache/otel invariants is the bulk of the work (~3–4 days). The resume model (`vm.rs:~1480`, `scheduler.rs:627`) **forbids** the cheaper "just suspend the native frame" shortcuts — that path resumes into bytecode, not into the middle of a Rust `for`, so there is no way around the lift. Gate it on `in_async_context()` so the sync top-level path is untouched.
- **Provider drift — mitigated, not eliminated.** Spawning the provider's own `complete_async` via `Arc` (step 2) avoids re-implementing send/parse, but retry/backoff moves into `run_async_with_retry` and must stay behaviorally identical to `complete_with_retry` (`builtins.rs:~5072`). The FakeProvider retry test is the guard.
- **New blocking point.** The park-on-IO branch (step 5) is the only place the VM thread blocks. It must park *only* when zero Ready tasks remain, re-checked each pass, or it will stall runnable tool/VM work. Keep the `check_interrupt()` cadence so Ctrl-C still works.
- **otel span timing shifts.** The CLIENT span now brackets a yield, so its self-time includes park time. Arguably more accurate; flag it so it isn't read as a regression. Spans are `Rc` and stay on the VM thread.
- **Virtual clock vs real time.** Real HTTP waits do not advance `virtual_now`. `async/timeout`'s virtual-clock semantics around a network call need explicit handling — an IO-bound task must not be force-woken by clock advance while its request is genuinely in flight. **⚠️ §8 B3 upgrades this from caveat to blocker:** with no real sleeper to pace against, the all-blocked branch jumps `virtual_now` straight to the timeout deadline and returns `TimedOut` regardless of whether the response is landing (`scheduler.rs:550-576`), so `async/timeout` around any `agent/run` **always spuriously fires**. Step 5 must short-circuit before the sleep/timeout advance and wire a **real wall-clock** timeout for `AwaitIo`.
- **Cancellation is best-effort.** `async/cancel` marks the task failed; the in-flight tokio future runs to completion and its response is dropped (step 9). No socket abort in v1.
- **Backpressure.** Unbounded `async/spawn` of agents opens unbounded concurrent sockets. v1 has no worker-pool cap; add a semaphore around `shared_rt().spawn` if needed.
- **wasm.** No real tokio runtime/threads. The whole `AwaitIo` path is `#[cfg(not(target_arch = "wasm32"))]`; in wasm `agent/run` stays synchronous.
- **Out of scope for v1:** `llm/stream`, `llm/pmap`, `batch_complete`, and tool handlers that do their own blocking IO.

**Effort:** ~6–9 engineering days. Phase 1 (steps 1–5 + spike) ~2 days and de-risks the whole thing. Phase 2 (`llm/chat-once`, step 6) ~1.5 days. Phase 3 (the lift, steps 7–8) ~3–4 days. Phase 4 (tests + live benchmark + in-flight counter, step 10) ~1.5 days.

---

## 7. How `workflow/foreach |parallel` sits on top

Once `agent/run` is a cooperative yield rather than a thread-blocking call, parallel workflow combinators become **free composition** — they do not need their own concurrency machinery. The pattern is exactly the acceptance-oracle shape:

```sema
;; workflow/foreach with :parallel lowers to spawn-all + async/all
(workflow/foreach items
  (fn (item) (agent/run agent item))
  :parallel true)

;; ... is sugar for:
(async/all
  (map (fn (item) (async/spawn (fn () (agent/run agent item)))) items))
```

Because each spawned `agent/run` now parks on `AwaitIo` instead of freezing the VM thread, `async/all` already gets true overlap from the scheduler — `workflow/foreach |parallel` is a thin macro that emits the spawn-all/`async/all` shape and collects results **by index** (so output order matches input order even though completion order is nondeterministic). No new runtime, no new yield reason, no Send-boundary work at the workflow layer.

This is the payoff of doing the work at the scheduler/yield level rather than inside any one combinator: **every** parallel construct — `async/all`, a future `llm/pmap` rebuilt on `AwaitIo`, `workflow/foreach |parallel`, fan-out/fan-in graphs — inherits overlap from the single `AwaitIo` mechanism. Build the yield once; the combinators are sugar.

---

## 8. Verification addendum (review 2026-06-23)

A multi-agent verification pass checked every cited `file:symbol` and adversarially stress-tested the load-bearing claims against the actual tree. **Verdict: the foundational thesis is correct and the scheduler-half plumbing (Steps 1, 3, 4, 8) is accurate and drops in — but the plan is *not implementable as written*. Five blocker-class issues must be resolved, and the bulk of the real risk is in Phase 2 (`llm/chat-once`), not Phase 3.**

### Confirmed sound (no change needed)
- The diagnosis (`block_on` at `anthropic.rs:429` parks the one VM thread; `async/spawn`+`agent/run` cannot overlap today).
- The yield/park mechanism. `YieldReason`@19, `wake_blocked_tasks`@149 (`WakeAction::{Pending,Resume(Value),Fail(String)}`), deadlock@582, `replace_stack_top`@627, the `AsyncYield` nil-placeholder/pc-save model (`vm.rs:1490-1497`), `ReinstallGuard`@446-489 — all verified. The proposed `AwaitIo` wake arm mirrors `AwaitPromise` cleanly.
- The resume model genuinely **forbids** suspending the `run_tool_loop` Rust frame → the Sema lift is *necessary*, and a register_fn_ctx leaf-yield (the shipped `async/await` pattern, `async_ops.rs:112-135`) is correctly intercepted; the nil return never propagates and resume values are **per-task** (no cross-task mis-delivery — `scheduler.rs:225/626-627`).
- `ChatRequest`/`ChatResponse` are genuinely `Send` (they carry `serde_json::Value`, never `Rc`-based `sema_core::Value`) — the Send-boundary *thesis* holds, and `Rc<IoHandle>` never needs `Send` (the `Scheduler` lives in a `thread_local`, imposing no `Send` bound).
- FakeProvider's recorder is `Arc<Mutex>`, **not** thread_local (`fake.rs:84`) → cross-thread recording is safe (the feared "invisible recorder" break does **not** occur). Cancel's best-effort dropped-`oneshot` story holds. Cache-hit zero-usage short-circuits before any provider call (`builtins.rs:4790-4816`). Tool *invocation* is genuinely Sema-callable (`call_callback`, `builtins.rs:5650`).

### Blockers — resolve before Phase 2
- **B1 — `complete_async` is not on the `LlmProvider` trait.** It is an *inherent* method on each concrete struct (`anthropic.rs:13/129`); the trait (`provider.rs:6`) exposes only sync `complete()`, which `block_on`s the provider's *own* runtime. `Arc<dyn LlmProvider>` can't reach `complete_async`; spawning `complete()` on `SHARED_RT`'s async pool **panics** (nested runtime, `http.rs:8-17`). **Decision required:** (a) add an async method to the trait — `fn complete_boxed(&self, req) -> Pin<Box<dyn Future<Output=Result<ChatResponse, LlmError>> + Send + '_>>` (or `async-trait`) — implemented across all 4 providers **and** FakeProvider; **or** (b) `shared_rt().spawn_blocking(move || provider.complete(req))` (overlap still works; abandons "no re-implementation", keeps retry on the sync path, and adds a blocking-pool size cap to size). This is the plan's central de-risking pillar and it does not exist as described.
- **B2 — otel span stack corrupts under task interleaving.** The otel `STACK` is one `thread_local Vec<Context>`, parented to `STACK.last()` and popped **blindly LIFO** on `Drop` (`sema-otel/src/imp.rs:80-82, 601, 523-528`); conversation/session/user ids are single-slot with LIFO restore. The agent span brackets the yield by design, so when task A parks mid-round and task B opens its span on the same stack, B mis-parents under A's in-flight request and out-of-LIFO completion pops the wrong task's span — permanent desync. "Spans stay on the VM thread" is the **cause**. **Fix:** add a per-task otel TLS snapshot/restore (swap `STACK` + the id slots) on park/resume, mirroring `replace_stack_top`; store the snapshot in the `Task` struct. Add a §5 test asserting two interleaved agents each parent their chat spans to their **own** agent span with their **own** `gen_ai.conversation.id`.
- **B3 — `async/timeout` over an `AwaitIo` call always spuriously fires.** With no real sleeper, the all-blocked branch jumps `virtual_now` to the timeout deadline and returns `TimedOut` regardless of whether the response is landing (`scheduler.rs:550-576`). **Fix:** Step 5 must short-circuit before the sleep/timeout advance and wire a real wall-clock timeout for in-flight IO tasks. Define park precedence when Sleep tasks coexist with `AwaitIo` tasks.
- **B4 — `Value`↔`ChatMessage` conversions are lossy.** `sema_list_to_chat_messages`/`chat_messages_to_sema_list` (`builtins.rs:5390/5423`) carry only `:role`/`:content` and drop `tool_calls`/`tool_call_id`/`tool_name`. The correlation invariant the lift must preserve has **no** Sema representation today. **Fix:** net-new full-fidelity marshaling natives that round-trip `assistant_with_tool_calls` and `tool_result`.
- **B5 — `do_complete` is not cleanly separable.** Step 6b's `cache_lookup(&req)` doesn't exist as a function; cache-key model resolution, cassette record/replay, fallback-chain per-provider substitution, `set_serving_provider` stamping, and span dispatch are all interwoven with the provider call (`builtins.rs:4731-5139`). **Fix:** refactor `do_complete` to extract a synchronous on-VM-thread cache/cassette/key-resolution stage from the async wire stage *before* writing `llm/chat-once`.

### Majors
- **Thread-local retry knobs lost on the worker.** `RETRY_BASE_MS`/`NETWORK_MAX_RETRIES` (`builtins.rs:5015-5031`) are thread_local; retry inside the `SHARED_RT` future reads the worker's defaults, not the test's `set_retry_base_ms(0)` → the deterministic FakeProvider retry oracle would actually sleep. Capture and cross the Send boundary alongside `max_retries`.
- **Mis-priced usage.** `track_usage` prices by the thread_local serving-provider stamp set *during* the provider call (`builtins.rs:245`); a spawned call never sets it on the VM thread. Thread the serving-provider name back in the return (the plan returns only `ChatResponse`).
- **OpenAI `DROP_TEMPERATURE` self-heal dropped.** That 400-retry-once compat lives only in the sync `complete()` wrapper (`openai.rs:634-653`), not in `complete_async` — bypassing it regresses a shipped self-healing path (CLAUDE.md invariant).
- **Retry relocation = the "provider drift" the architecture claimed to avoid.** A hand-rolled `run_async_with_retry` must stay behaviorally identical to `complete_with_retry`; the FakeProvider retry test is the only guard.
- **`blocking_sleep_ms` has no `check_interrupt()` cadence to "mirror"** (`async_signal.rs:235-244` is one uninterruptible `thread::sleep`). The park-on-IO wait must be *built* with its own timeout/interrupt-poll loop (e.g. `Condvar::wait_timeout` polling `check_interrupt`).
- **FakeProvider has no latency knob** → the "in-flight ≥ 2" deterministic gate is unprovable (fake futures complete instantly). Add a per-reply `delay_ms` honored by the async path.
- **Fallback chain dropped.** Spawning one named `Arc<dyn LlmProvider>` loses `llm/with-fallback` semantics — scope out for v1 or capture the `FallbackEntry` list.
- **Port completeness.** Step 7 under-specifies what `run_tool_loop` actually does beyond echo/correlation: conv/session/user scope + `agent_span` + `apply_call_telemetry_agent`, per-tool `tool_span` + `set_tool_io` + `record_error`, `on_tool_call` start/end callbacks with 200-char-safe truncation, `MAX_CONSECUTIVE_TOOL_ERRORS=5`, `execute_tool_call`'s arg-validation/error-as-feedback, and `set_trace_io` rollup. (Streaming, `json_mode`, `tool_choice`, parallel-tool config are genuinely **not** in `run_tool_loop`, so the port correctly omits them.)

### Minors / nits
- `IoHandle` (holds `Box<dyn FnMut>`) is not `Debug`/`Clone` → manual `impl Debug` needed or `#[derive(Debug, Clone)]` on `YieldReason` (`async_signal.rs:18`) won't compile.
- Step 6a's `take_resume_value()` guard is **vestigial** under `CALL_NATIVE` (value arrives via `replace_stack_top`; native not re-entered) — internally inconsistent with §2; keep only for symmetry.
- The otel guards are **not `Rc`** (thread_local-stack-bound `opentelemetry::Context`); `_tele` is in `agent/run` (`builtins.rs:2335`), not `run_tool_loop`.
- `complete()` takes `ChatRequest` **by value**, not `&ChatRequest`.
- No existing wasm cfg in `sema-llm` to "match" — net-new gating; `SHARED_RT`'s multi-thread runtime can't build on wasm.
- `prelude.rs` is 110 lines of macros, **zero functions** — a large agent loop there is unusual placement; a loaded `.sema` module would be cleaner. Step 2's `Arc`-ification touches ~15 call sites (`&dyn` → `Arc<dyn>`): mechanical but wider than "localized".
- Line cites are otherwise exact or within ~10 lines throughout.

### Revised de-risking sequence
1. **Run the §5 spike first (unchanged, ~2 days).** It uses a timer leaf (`llm/io-sleep-once`), touches none of B1–B5, and proves the one genuinely novel claim — overlap across the per-task-VM scheduler. Gate everything on it showing ~1 s for five 1 s sleeps.
2. **Resolve B1** (trait decision: async trait method vs `spawn_blocking`) — this changes the retry, compat, and FakeProvider story, so decide it before any Phase-2 code.
3. **B2 (per-task otel TLS snapshot/restore)** and **B3 (real-clock IO timeout)** are scheduler/core changes independent of the LLM lift — land them next, each with a focused test.
4. **B5 (extract a synchronous cache/cassette/key stage from `do_complete`)**, then **B4 (full-fidelity message marshaling natives)** — these are the real Phase-2 cost.
5. Only then write `llm/chat-once` (Step 6) and the Sema loop (Step 7), carrying retry-base + serving-provider across the boundary and preserving the per-tool span/callback obligations.

**Revised effort:** Phase 1 (spike + scheduler plumbing + B2/B3) ~3–4 days; Phase 2 (B1 decision + B5 refactor + B4 marshaling + `llm/chat-once`) is the dominant cost and larger than the original ~1.5-day estimate; Phase 3 (Sema loop) ~3–4 days as stated. The "~6–9 day" total is optimistic — budget the Phase-2 LLM-side surface explicitly.
