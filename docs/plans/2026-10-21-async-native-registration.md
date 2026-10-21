# Async native function registration for embedders

**Status:** Approved design; no API in this document is implemented by this PR.
The owner approved D1-D5 on 2026-10-21. Implementation follows in a separate PR.

## Problem and current behavior

[Kameko's embedding feedback](https://github.com/sema-lisp/sema/pull/172#issuecomment-6303419534)
asks for easier async host function registration. Public
`Interpreter::register_fn` accepts a synchronous callback:

```rust,ignore
Fn(&[Value]) -> Result<Value, SemaError>
```

An async closure returns a future, so it does not satisfy this signature.
Calling `block_on` inside the callback blocks Sema's interpreter thread and can
conflict with an existing Tokio runtime. The internal runtime already supports
external futures, suspension, cancellation, and completion decoding. The
missing part is an embedding API that uses those mechanisms safely.

Sema `Value` objects and errors containing them belong to their creating
thread. Keywords and symbols use a thread-local interner; heap values use
non-atomic reference counts. An async API must not move them into worker
futures. The `Value` thread-affinity fix (#175) is a prerequisite: its `!Send`
marker is what makes the `Send` bounds below reject `Value` and `SemaError` in
worker data.

## Decision status

| ID | Decision | Approved design | Other choices and effect |
|---|---|---|---|
| D1 | Callback shape | Three callbacks: `prepare`, `run`, `finish` (see below). | A typed `Input: FromValue` / `Output: IntoValue` form (plan 2026-03-11 sections 2 and 3) collapses this to one callback but needs the conversion traits first. It can wrap D1 later without changing the runtime adapter. |
| D2 | Error mapping | Host `Error` is rendered to text on the executor and raised as `SemaError::Io("<name>: <text>")` on the interpreter thread, matching `http/*`. Each `ExternalFailureCode` keeps a distinct, documented message prefix so cancellation, executor rejection, deadline, worker panic and decode failures are distinguishable in `catch`. | Reusing the current `IoDecoder` mapping collapses every failure code into `SemaError::eval` text and loses classification. A new `SemaError` variant is cleaner but widens the public error enum. |
| D3 | Completion kind | One shared `HOST_ASYNC_COMPLETION_KIND` constant for every host-registered async function. The kind is only checked for self-consistency when a completion is delivered, so per-registration kinds buy nothing. | Per-registration kinds need an allocator and a registry for no observable benefit. |
| D4 | Cancellation | The adapter wraps the host future in a biased `tokio::select!` against a one-shot cancel signal owned by the wait's `CancelHook`. Cancellation drops the host future at its next poll. Late completions are discarded by the existing wait generation. | Relying on the executor's abort handle does not work: the process pool drops it for async dispatches by design. |
| D5 | wasm32 | `register_async_fn` is compiled on every target but the default wasm32 runtime uses `NullExecutor`, which rejects every submission (`SubmitErrorKind::Capacity`), so a call fails with the existing executor-rejection error (`ExternalFailureCode::Rejected`). A browser host that installs its own `IoExecutor` can use it. | `cfg`-gating the method hides it from hosts that do supply an executor. |

## Proposed API: prepare, run, finish

Add `Interpreter::register_async_fn` with three callbacks. Their responsibilities
and thread requirements are explicit:

| Callback | Runs on | Input and output |
| --- | --- | --- |
| `prepare` | Interpreter thread | Validate `&[Value]` and produce owned `Input`. |
| `run` | Sema's process-wide Tokio pool | Accept `Input`, construct and await a future that returns owned `Output` or a host error. |
| `finish` | Interpreter thread | Convert `Output` into `Value`, or return a `SemaError`. |

Proposed bounds, not final implementation syntax:

```rust,ignore
pub fn register_async_fn<Input, Output, Error, Prepare, Run, Fut, Finish>(
    &self,
    name: &str,
    prepare: Prepare,
    run: Run,
    finish: Finish,
)
where
    Input: Send + 'static,
    Output: Send + 'static,
    Error: std::error::Error + Send + 'static,
    Prepare: Fn(&[Value]) -> Result<Input, SemaError> + 'static,
    Run: Fn(Input) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Output, Error>> + Send + 'static,
    Finish: Fn(Output) -> Result<Value, SemaError> + 'static;
```

`Run` can be shared by concurrent calls; the future and each input/output belong
to one call. `prepare` and `finish` do not require `Send`. The adapter holds
`run` as an `Arc<Run>` captured by a per-call `FnOnce` job, and `finish` as an
`Rc<Finish>` held by the completion decoder on the interpreter thread.

For example, an application can register a delayed keyword constructor:

```rust,ignore
// Proposed API. The example also requires Tokio's time support.
use sema::{Interpreter, SemaError, Value};
use std::{convert::Infallible, time::Duration};

let interp = Interpreter::new();
interp.register_async_fn(
    "host/delayed-keyword",
    |args| {
        if args.len() != 1 {
            return Err(SemaError::eval("expected one string argument"));
        }
        args[0]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))
    },
    |name: String| async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok::<String, Infallible>(name)
    },
    |name| Ok(Value::keyword(&name)),
);
let value = interp.eval_str("(host/delayed-keyword \"ready\")")?;
assert_eq!(value, Value::keyword("ready"));
```

The future handles a `String`. `Value::keyword` runs after completion on the
interpreter thread, so it uses that thread's interner. The same pattern applies
to HTTP bodies, database rows, and structured JSON.

## Which runtime runs `run`

On native targets the interpreter's runtime attaches to Sema's process-wide
Tokio pool through `sema_io::process_executor()` (`build_runtime` in
`crates/sema-eval/src/eval.rs`). The adapter submits the job through
`PreparedExternalOperation::interruptible_async`, and the pool spawns it with
`io_spawn`, so `run` executes inside a Tokio runtime context. This is why the
example can construct a Tokio timer without the host providing a runtime.

Consequences to document with the API:

- `run` executes on Sema's pool, not the host's Tokio runtime. A `tokio::spawn`
  inside `run` lands on Sema's pool. Resources created on the host's runtime
  follow Tokio's cross-runtime rules.
- `io_block_on` and any other `block_on` must not be called inside `run`; the
  seam asserts against blocking from a runtime worker.
- The host's own ambient Tokio runtime is not a supported context for
  `eval_str` until the full evaluation path has been tested, including
  synchronous builtins called by the same script.
- On wasm32 the default runtime uses `NullExecutor` and rejects External waits
  (D5).

## Evaluation and lifecycle

Calling the function suspends the calling Sema task while the future runs.
Other runnable Sema tasks can continue. `eval_str` still blocks its Rust caller
until evaluation finishes; registering an async callback does not make
`eval_str` an async Rust method. Hosts that need a responsive UI can create an
interpreter on a dedicated thread and communicate using owned Rust messages.

The adapter is the existing stdlib async offload path, generalized. Today
`external_io_async_try` in `crates/sema-stdlib/src/runtime_offload.rs` already
has this shape: a `Send` `FnOnce() -> Fut` job, an interpreter-thread
`FnOnce(T) -> Result<Value, SemaError>` decoder (`IoDecoder`), a single-shot
`IoOffloadContinuation`, and a `SelectCancelHook`. It is `pub(crate)`.
Implementation promotes that path to a public-safe adapter instead of writing a
second one, so host functions get the same cancellation and teardown behavior
as `http/*`, and the stdlib callers keep pinning it.

Use `NativeFn::with_context_result`, `NativeOutcome::Suspend`,
`WaitKind::External`, and `PreparedExternalOperation::interruptible_async`.
Keep preparation and the completion decoder on the interpreter thread. Box
the owned result into the existing `SendPayload`; decode it through the typed
adapter before calling `finish`. Use the existing runtime's wait generation,
resource accounting, and cancellation handling. Use the single
`HOST_ASYNC_COMPLETION_KIND` (D3).

### Cancellation and shutdown

The process executor deliberately drops the Tokio abort handle for async
dispatches (`ProcessPool::submit` in `crates/sema-io/src/executor.rs`).
Cancellation reaches the job only through the wait's `CancelHook` on the
interpreter thread. The adapter therefore wraps the host future in a biased
`tokio::select!` against a one-shot cancel signal (D4); firing the signal makes
the job drop the host future and complete with a cancelled result.

- Cancellation means the host future is dropped at its next poll. Hosts must
  write drop-cancel-safe futures. It does not undo remote side effects.
- A future stuck in a blocking call, and any task it detached with
  `tokio::spawn`, is not stopped. Future polling must remain cooperative;
  CPU-bound loops and blocking I/O inside `run` cannot be made interruptible
  by this API.
- `Interpreter::drop` runs a bounded runtime shutdown with a 2 s deadline and
  the pool drains per-runtime in-flight jobs. A non-cooperative host future
  makes interpreter drop wait up to that deadline; shutdown then reports
  `ExecutorShutdown::DeadlineExceeded` and the job is detached, not joined.
- Late completions after cancellation are discarded by the existing wait
  generation check. No new bookkeeping is needed.

Use the existing runtime's panic/error mapping where available, and specify
behavior under the crate's aborting panic profile. Under the unwind profile a
panicking host future surfaces as `ExternalFailureCode::WorkerPanic`. Do not
promise that `catch_unwind` catches aborts.

### Error mapping

Convert worker errors to owned error text on the executor and construct the
`SemaError` on the interpreter thread, including the registered function name
(D2). Preparation and conversion failures retain their local `SemaError`
details. The current `IoDecoder` collapses every `ExternalFailureCode` into
`SemaError::eval` text; the public adapter needs its own decoder so that
cancellation, executor rejection, deadline, worker panic and decode failures
each keep a distinct, documented message prefix and are not mistaken for an
ordinary host function error.

## Call-site restrictions

A function registered this way is a runtime-only native: its legacy value ABI is
the "requires runtime invocation" stub installed by
`NativeFn::with_context_result`. Document, next to the registration, the places
where a call raises an error instead of suspending:

- Macro transformers. Expansion cannot suspend; the call fails with
  "macro transformer cannot suspend during expansion" before any job is
  admitted (see `runtime_macro_transformer_rejects_external_wait_before_job_admission`
  in `crates/sema-eval/src/eval.rs`).
- Synchronous `apply` / `call-with-values` paths, which reject runtime-only
  natives with an actionable error (`is_runtime_only_native` in
  `crates/sema-stdlib/src/list.rs`).
- Debug evaluation, restricted/test evaluation, and synchronous wasm
  evaluation, which all report that they cannot suspend.
- Synchronous HOF fast paths decline a callback that may suspend and fall back
  to the cooperative path; this is transparent to the host.

`prepare` and `finish` run inside an active runtime quantum. They must not call
`eval_str`, `eval`, or `io_block_on`; the same rule already applies to
synchronous native functions.

## Captured state and garbage collection

The adapter must follow the existing native-function GC rule (Invariant I2):
boxed callback closures must not strongly capture `Value`, `Env`, or objects
that contain them. `Send` bounds prevent those captures in `run`, but cannot
enforce this rule for local `prepare` and `finish` closures. Document that
restriction next to registration, including the same restriction on
synchronous native functions.

Any adapter-owned Sema state must use traceable payloads and continuations.
Implement `Trace` for the completion decoder and continuation with an explicit
account of what they retain: the decoder holds `Rc<Finish>` and the function
name, the continuation holds the name only, and both report no GC edges,
matching `IoDecoder` and `IoOffloadContinuation`. Do not use an untraced `Rc`
closure to retain interpreter values. A later typed conversion API may reduce
callback boilerplate, but it must not hide this lifetime requirement.

## Scope and implementation steps

1. Generalize `external_io_async_try` and its decoder, continuation and cancel
   hook out of `sema-stdlib` into a public-safe adapter over the existing
   executor and runtime ABI. Keep the stdlib callers on it. Keep internal
   scheduler types out of the public method signature.
2. Add `Interpreter::register_async_fn` in `crates/sema/src/lib.rs` wrapping
   the adapter with `NativeFn::with_context_result`, `Arc<Run>` and
   `Rc<Finish>`. Add lifecycle, error, call-site and GC documentation. Leave
   existing `register_fn` behavior unchanged.
3. Add the runnable embedding example and acceptance tests below. Update
   `website/docs/embedding.md` and the changelog.
4. Review ergonomics before adding convenience overloads. Native local
   `!Send` futures, caller-provided executors, browser execution, and an async
   host evaluation method need separate contracts and are outside this change.
   The typed form (D1 alternative) belongs to plan 2026-03-11 sections 2 and 3.

No new scheduler, global executor, unsafe `Send` implementation, or internal
interpreter accessor is required. This design is independent of the MCP API
proposal (#176); both depend on the thread-affinity fix (#175).

## Acceptance checks

- A fixture with only public embedding dependencies registers and calls an
  async function. Existing synchronous registrations continue to work.
- A controlled pending future lets another Sema task run. Use explicit test
  signals rather than elapsed-time assumptions to prove suspension.
- N concurrent calls started from `async/spawn` overlap on the executor; the
  shared `run` is invoked once per call.
- Preparation and completion run on the interpreter thread. The executor job
  can construct a Tokio timer. A returned keyword resolves correctly even
  when the worker has interned unrelated names.
- Compile-fail checks reject inputs, outputs, errors, and worker captures
  containing `Value`; local error construction and owned-data transfer work.
- A `prepare` error submits no job and leaves `runtime_live_task_count` at 0.
- Calling the function inside a macro transformer raises the documented
  expansion error; no job is admitted.
- `async/timeout` around the host call cancels it: the host future is dropped
  (observed through a drop guard) and the error is the documented cancellation
  classification, distinct from a host error.
- A host future that panics under the unwind profile surfaces as a worker-panic
  error, not an abort.
- Cancellation, host errors, conversion failures, executor rejection, and
  late completion leave no live task, wait, or resource gate behind
  (`runtime_live_task_count` and `runtime_resource_gate_count` both 0).
  Include interpreter shutdown while a cooperative future is pending, and
  confirm shutdown returns within its deadline when the future is not.
- GC tests cover a pending operation and collection after cancellation or
  completion. No hidden closure capture keeps an interpreter graph alive.
- Run the existing async/runtime suites and rebuild the bundled web runtime
  if shared runtime source changes, even though the public API starts native-only.

## Code references

- Public registration: `crates/sema/src/lib.rs`.
- Native callback ABI: `crates/sema-core/src/runtime/native.rs` and
  `crates/sema-core/src/value.rs`.
- Jobs and completion: `crates/sema-core/src/runtime/executor.rs` and
  `crates/sema-core/src/runtime/completion.rs`.
- Reference async offload path: `crates/sema-stdlib/src/runtime_offload.rs`
  (`external_io_async_try`, `IoDecoder`, `IoOffloadContinuation`,
  `SelectCancelHook`).
- Process executor and cancellation delivery: `crates/sema-io/src/executor.rs`.
- Evaluation entry points and interpreter shutdown: `crates/sema-eval/src/eval.rs`.
- Related conversion work: [earlier embedding API plan](2026-03-11-embedding-api-improvements.md).
  Its coroutine section is superseded; this proposal uses the current runtime.
  Its sections 2 and 3 (`register_fn_typed`, `IntoValue`/`FromValue`) remain
  the home of the typed form.
