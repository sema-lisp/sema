# One I/O pool behind one seam (runtime consolidation, Slice A) — ADR #69

**Status:** IMPLEMENTED (steps 1-7 landed 2026-07-03 on
feature/true-async-agent-loop; both oracles green, allowlist exactly as
designed below). Design: 4 investigators incl. empirical tokio probes →
synthesis → 2-lens adversarial review → finalize, 2026-07-03.
Goal (user-stated): *"the codebase must not sprawl ad-hoc mechanisms for what the
scheduler should be handling"* — one park/wake mechanism (the scheduler + `AwaitIo`),
one executor seam, N thin platform backends.

## Why (the sprawl is already costing us)

19 tokio-runtime-creation sites today. The core duplication: `SHARED_RT`
(sema-llm/http.rs:51) and `STDLIB_SHARED_RT` (sema-stdlib/async_rt.rs) are identical
pools split only by crate layering; every provider *instance* carries its own
`BlockingRuntime` (create_runtime, http.rs:65); the sync http path holds a
thread-local full runtime (`HTTP_RUNTIME`, http.rs:7); `http/serve` spawns its own
thread+runtime (server.rs:1306). Exhibits of the failure mode: the wasm
replay-with-cache http hack (a second suspend/resume mechanism with re-execution
semantics) and the sema-web streaming deferral ("native llm/stream drives its own
runtime and the SSE channel's blocking send panics inside it").

## Empirical foundation (tokio 1.50.0 — the workspace-resolved version; probes run,
not doc-recalled)

- **(a)** `block_on` (via static `OnceLock<Runtime>` or `Handle`) from a
  `spawn_blocking` thread **of the same runtime**: **OK** — the literal consolidated
  production shape (VM thread → `io_spawn_blocking(run_fallback_retry)` →
  `provider.complete()` → `io_block_on(reqwest)`).
- **(b)** `block_on` from an async **worker** thread: **PANICS** ("Cannot start a
  runtime from within a runtime…" — full message continues past this prefix; pin
  tests assert panic-occurs, not exact wording).
- **(c)** `block_on` from a plain OS thread (the VM thread): OK.
- **(d)** 64 concurrent `spawn_blocking→block_on` units on a 1-worker runtime: all
  complete — `block_on` drives the future **on the calling thread**; workers supply
  only the reactor/timer. (Consequence: the block_on tier never touches a pool
  worker → identity oracle must count seam entries, not thread names, for that tier.)
- **(h)** Nested `spawn_blocking→block_on→spawn_blocking` **deadlocks at blocking-cap
  == N** and completes at 2N: reqwest's GaiResolver DNS transiently needs +1 blocking
  slot per in-flight `block_on`. Today that lands on the *provider's own* pool; after
  consolidation it lands on the one pool → **a real regression at burst fan-out unless
  mechanically prevented** → admission control below.

## The seam — `sema-core/src/io_backend.rs` (tokio-free, all targets)

Sixth instance of the house type-erased-registration idiom (precedents:
`set_eval_callback`, otel task callbacks, usage-scope callbacks,
`set_blocking_sleep_callback`, `set_interrupt_callback`). Divergences, both argued: a
process-global `OnceLock` instead of thread-locals (the pool is reachable from pool
threads and the server thread; precedent: `IO_SIGNAL` in async_signal.rs), and one
trait object for the three ops (one backend identity, like the fused otel triple).

```rust
/// One-shot cancel hook returned by io_spawn (tokio AbortHandle::abort on native;
/// AbortController.abort on a future wasm backend). Slots into
/// IoHandle::with_abort one-for-one where join.abort_handle() sits today.
#[cfg(not(target_arch = "wasm32"))] pub type AbortHook = Box<dyn FnOnce() + Send>;
#[cfg(target_arch = "wasm32")]      pub type AbortHook = Box<dyn FnOnce()>;

#[cfg(not(target_arch = "wasm32"))]
pub type BoxIoFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub type BoxIoFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

pub trait IoBackend: Send + Sync {
    /// Spawn a future on the pool; returns a one-shot abort hook.
    fn spawn(&self, fut: BoxIoFuture) -> AbortHook;
    /// Offload a synchronous closure to the pool's blocking tier.
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send>);
    /// Drive a boxed future to completion ON THE CALLING THREAD using the
    /// pool's reactor. NATIVE-ONLY semantics: a wasm backend panics here
    /// (every sync blocking consumer is cfg(not(wasm32))-gated).
    fn block_on_boxed(&self, fut: Pin<Box<dyn Future<Output = ()> + '_>>);
}

static IO_BACKEND: OnceLock<Box<dyn IoBackend>> = OnceLock::new();
pub fn set_io_backend(b: Box<dyn IoBackend>) -> bool;   // first-wins, idempotent
pub fn io_backend() -> Option<&'static dyn IoBackend>;
// Generic sugar: io_block_on<T>(fut) -> T boxes `async { *slot = Some(fut.await) }`
// so non-Send / non-'static futures (provider &self borrows, on_chunk callbacks
// over Sema values) work unchanged — block_on drives on the caller thread.
```

**Contract (module docs + pin tests):** `io_block_on` is legal from the VM thread,
plain OS threads, and `io_spawn_blocking` closures; PANICS from `io_spawn` futures /
any async-driver thread. A `block_on`'d future may transiently need at most ONE
blocking slot of its own (GaiResolver DNS); never nest a second
spawn_blocking-and-wait level inside one.

## The backend — new leaf crate `crates/sema-io` (~100 lines; deps: sema-core + tokio)

- THE pool: `OnceLock<Runtime>`, `new_multi_thread().enable_all()`
  (`.enable_all` required: tokio::process driver for shell, timers for sleep-once),
  `.max_blocking_threads(512)`, `.thread_name_fn(|| "sema-io-{n}")`.
- **Admission control (critique-mandated mechanism, not guidance):** an
  `OFFLOAD_SEM: Semaphore(448)` acquired around `spawn_blocking` offload units,
  reserving 64-slot headroom so each in-flight `block_on`'s depth-1 DNS need can
  always be satisfied → the probe-(h) deadlock is structurally unreachable, and
  behavior at realistic fan-out is identical (excess offloads queue briefly).
  Retry-backoff `thread::sleep` (builtins.rs ~6700) correctly holds its permit for
  the backoff duration — accounted, documented.
- `sema_io::install()` — idempotent, first-wins. Called from `register_stdlib`
  (native branch), `register_llm_builtins`, and `reset_runtime_state`, so lib tests
  without the full interpreter still get THE one pool.
- **Sanctioned entry:** `sema_io::{io_spawn, io_spawn_blocking, io_block_on}`
  install-then-delegate wrappers are the ONLY entry for native crates
  (sema-core's raw fns stay for wasm/core-level use); the conformance scan forbids
  direct `sema_core::io_*` calls outside sema-io.
- Wasm backend (later, Slice B): implements the two spawn ops over
  fetch/JS-promises (`AbortHook` wraps `AbortController.abort`), panics in
  `block_on_boxed`; retires the replay-with-cache hack.

## Oracles

1. **Source-conformance test** (`crates/sema/tests/runtime_conformance_test.rs`) —
   **RED today (≈8 production violations), GREEN at step 7.** Walks
   `crates/*/src/**/*.rs` (workspace root via `env!("CARGO_MANIFEST_DIR")/../..`;
   precedent: web_dev_server_test.rs / builtin_doc_coverage). After comment-stripping,
   OR-matches path-free tokens `Runtime::new(`, `new_multi_thread(`,
   `new_current_thread(`, `#[tokio::main]` — plus direct `sema_core::io_spawn/
   io_block_on/io_spawn_blocking` outside sema-io. Explicit allowlist (path +
   max-count + reason): `sema-io/src/lib.rs` (the blessed backend),
   `sema-otel/src/imp.rs` (isolated OTLP export reactor — deliberate),
   `sema/src/main.rs` (subcommand entry-point drivers ARE main()),
   `sema-mcp/*` + `sema-notebook/src/bridge.rs` (out of this slice, tracked),
   `#[cfg(test)]` code.
2. **Pool-identity test** (`crates/sema/tests/io_pool_identity_test.rs`) — drives all
   five offload kinds (async http, shell, async llm/complete via FakeProvider, sync
   llm/complete, sync http): spawn/spawn_blocking tiers assert their work ran on a
   `sema-io-*`-named thread; the block_on tier asserts a `BLOCK_ON_OPS` seam counter
   advanced (block_on drives on the caller thread — thread names can't observe it);
   `sema_io::pools_built() == 1` after all of it.
3. **Tokio-assumption pin tests** (unit tests in sema-io; green at creation — they
   pin the empirical contract the design rests on, so a future tokio upgrade that
   changes the rules fails loudly): (i) block_on-from-same-pool-spawn_blocking
   completes; (ii) block_on-from-worker panics (catch_unwind, any message); (iii)
   plain-thread block_on completes; (iv) probe-h nested fan-out deadlocks at cap N /
   completes at 2N under a watchdog; (v) probe-s admission-control oversubscription
   completes.
4. **Behavior gates per step:** vm_async_test, llm_fake_test, complete_async_test,
   agent_async_test + agent_async_breaker_test, http_concurrent_test,
   shell_concurrent_test, true_cancel_test, otel suites; full CI-equivalent at the
   end (`cargo test --workspace && make examples && make smoke-bytecode && make lint
   && make docs-check`), plus `make test-http` once (sync-client consolidation).

## Migration steps (each independently committable, tree green)

1. **Seam + backend + oracles, zero call sites moved.** io_backend.rs; crates/sema-io
   (pool, semaphore, wrappers, pin tests); install() wiring; conformance +
   pool-identity tests `#[ignore]`d with reason (RED verified first); Cargo.toml
   workspace pins 13→14 + AGENTS.md release-procedure count + publish.yml entry
   (after sema-core, before sema-stdlib/sema-llm).
2. **stdlib async offloads:** http_request_async (http.rs:192) + shell_async
   (system.rs:139) → `io_spawn`; AbortHook replaces `join.abort_handle()` one-for-one
   in `IoHandle::with_abort`; killpg layer composes AROUND the hook in stdlib,
   byte-identical. Delete async_rt.rs.
3. **stdlib sync http:** delete thread-local HTTP_RUNTIME; `rt.block_on` →
   `io_block_on`; fold thread-local HTTP_CLIENT into HTTP_SHARED_CLIENT (observable
   change: cross-thread connection pooling — benign; watch make test-http).
4. **sema-llm offloads:** do_complete_async_yield (builtins.rs:6181, covers agent
   step rounds) + embed offload (:3238) + io-sleep-once → `io_spawn_blocking` /
   `io_spawn` (now admission-controlled). Delete SHARED_RT.
5. **THE HAZARD STEP (probe-cleared):** delete per-provider BlockingRuntime fields
   (anthropic.rs:10, openai.rs:46, gemini, ollama, embeddings.rs:83/259); every
   `self.runtime.block_on(...)` → `io_block_on(...)`. MUST keep
   drive-on-caller-thread semantics (streaming on_chunk callbacks touch non-Send
   Sema values). Guard-comment openai's DROP_TEMPERATURE double block_on
   (openai.rs:635-648) as strictly-sequential-never-nested. Delete create_runtime /
   BlockingRuntime.
6. **server.rs:** prefer `io_spawn` of the Send+'static bind+serve future (keep the
   ready_tx port handshake; shutdown maps onto AbortHook). Escape hatch if graceful
   shutdown gets hairy: keep the dedicated thread but `io_block_on` there (probe c) —
   either way the ad-hoc runtime dies. In-file test fixtures (1456/1479) stay.
7. **Flip oracles green:** un-ignore conformance + pool-identity; final allowlist as
   in Oracle 1. Full CI-equivalent gate.

## Scope decisions (each argued, not lazy)

- **OUT sema-mcp** (block_on sites + client_auth): its private current-thread reactor
  has progress-only-during-block_on semantics; moving to an always-live pool changes
  keepalive/SSE-reconnect liveness — a behavior change, not a refactor. Own follow-up
  slice (feature/mcp-client). Pre-existing bug to file: mcp/* and llm/* panic inside
  `sema mcp` server's current-thread driver (probe b/f class) — neither fixed nor
  worsened here.
- **OUT sema-notebook bridge.rs** — runtime is a blocking-recv shim; right fix is
  std::sync::mpsc, no seam involvement.
- **OUT sema-lsp** — not ad-hoc: correctly reuses the main tower-lsp runtime's Handle
  from a plain thread.
- **OUT sema/src/main.rs** (7 subcommand drivers) — they ARE main(); hosting servers'
  drivers on the I/O pool inverts ownership.
- **OUT sema-otel OTEL_RT** — isolation is a feature: telemetry export must not
  contend with or tear down with user I/O.
- **OUT** sema-llm's unconditional tokio dep on wasm — pre-existing, separate cleanup.

## Residual risks (accepted, documented)

Tokio-version fragility (pinned by sema-io tests, re-established on CI Linux);
semaphore constants hand-picked (448/512 — queuing only beyond 448 concurrent
offloads); depth-1 headroom assumes no future block_on'd future needs >1 blocking
slot; pre-existing probe-f panic class (Sema callbacks inside a block_on'd poll
calling sync I/O) unchanged; always-live pool alters idle-thread footprint
(intended); server.rs step has its escape hatch.

**Follow-up landed (LLM-tier cancellation):** the "spawn_blocking LLM tier is
best-effort-cancel" limit is closed for the native providers. The completion/embed
wire stage moved from `io_spawn_blocking(sync closure)` to an `io_spawn`ed future
(`run_fallback_retry_async` over per-provider `complete_future`/`embed_future`
hooks) whose `AbortHook` slots into `IoHandle::with_abort` — cancel/timeout drops
the in-flight request like the http/shell tier (gate:
`llm_request_is_aborted_on_timeout` in `true_cancel_test.rs`). Consequence for the
admission story: spawned wire futures take NO offload permit (they pin no blocking
slot while suspended); the semaphore still guards the remaining blocking-tier
users — `io_spawn_blocking` and the new awaitable `io_offload_blocking`, which is
where sync-only providers (the `complete_future` default impl, e.g. FakeProvider)
run and where cancellation remains best-effort (result discarded, closure runs
out; gate: `sync_only_provider_cancel_is_best_effort`).
