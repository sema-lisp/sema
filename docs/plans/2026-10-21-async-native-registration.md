# Async native function registration for embedders

**Status:** Draft design; no API in this document is implemented by this PR.

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
futures. The `Value` thread-affinity fix is a prerequisite.

## Proposed API: prepare, run, finish

Add `Interpreter::register_async_fn` with three callbacks. Their responsibilities
and thread requirements are explicit:

| Callback | Runs on | Input and output |
| --- | --- | --- |
| `prepare` | Interpreter thread | Validate `&[Value]` and produce owned `Input`. |
| `run` | Existing I/O executor | Accept `Input`, construct and await a future that returns owned `Output` or a host error. |
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
to one call. `prepare` and `finish` do not require `Send`. The adapter calls
`run` on the executor, so constructing a Tokio timer or an I/O future occurs
in the correct runtime context. It should not create a runtime per call.

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

## Evaluation and lifecycle

Calling the function suspends the calling Sema task while the future runs.
Other runnable Sema tasks can continue. `eval_str` still blocks its Rust caller
until evaluation finishes; registering an async callback does not make
`eval_str` an async Rust method. Hosts that need a responsive UI can create an
interpreter on a dedicated thread and communicate using owned Rust messages.

Use `NativeFn::with_context_result`, `NativeOutcome::Suspend`,
`WaitKind::External`, and `PreparedExternalOperation::interruptible_async`.
Keep preparation and the completion decoder on the interpreter thread. Box
the owned result into the existing `SendPayload`; decode it through the typed
adapter before calling `finish`. Use the existing runtime's wait generation,
resource accounting, and cancellation handling.

Cancellation should request cancellation of the future and ignore late
completions. It does not undo remote side effects. Future polling must remain
cooperative; CPU-bound loops and blocking I/O inside `run` cannot be made
interruptible by this API. Use the existing runtime's panic/error mapping
where available, and specify behavior under the crate's aborting panic profile.
Do not promise that `catch_unwind` catches aborts.

Convert worker errors to owned error text on the executor and construct the
`SemaError` on the interpreter thread, including the registered function name.
Preparation and conversion failures retain their local `SemaError` details.
Cancellation and runtime failures keep their existing classification rather
than being mistaken for an ordinary host function error.

## Captured state and garbage collection

The adapter must follow the existing native-function GC rule: boxed callback
closures must not strongly capture `Value`, `Env`, or objects that contain
them. `Send` bounds prevent those captures in `run`, but cannot enforce this
rule for local `prepare` and `finish` closures. Document that restriction next
to registration, including the same restriction on synchronous native functions.

Any adapter-owned Sema state must use traceable payloads and continuations.
Implement `Trace` for completion decoders and continuations with an explicit
account of what they retain. Do not use an untraced `Rc` closure to retain
interpreter values. A later typed conversion API may reduce callback boilerplate,
but it must not hide this lifetime requirement.

## Scope and implementation steps

1. Implement a native-target adapter over the existing executor and runtime
   ABI. Keep internal scheduler types out of the public method signature.
2. Add public registration and lifecycle/error documentation. Leave existing
   `register_fn` behavior unchanged.
3. Add the runnable embedding example and acceptance tests below.
4. Review ergonomics before adding convenience overloads. Native local
   `!Send` futures, caller-provided executors, browser execution, and an async
   host evaluation method need separate contracts and are outside this change.

No new scheduler, global executor, unsafe `Send` implementation, or internal
interpreter accessor is required. This design is independent of the MCP API
proposal; both depend on the thread-affinity fix.

## Acceptance checks

- A fixture with only public embedding dependencies registers and calls an
  async function. Existing synchronous registrations continue to work.
- A controlled pending future lets another Sema task run. Use explicit test
  signals rather than elapsed-time assumptions to prove suspension.
- Preparation and completion run on the interpreter thread. The executor job
  can construct a Tokio timer. A returned keyword resolves correctly even
  when the worker has interned unrelated names.
- Compile-fail checks reject inputs, outputs, errors, and worker captures
  containing `Value`; local error construction and owned-data transfer work.
- Cancellation, host errors, conversion failures, executor rejection, and
  late completion leave no live task, wait, or resource gate behind. Include
  interpreter shutdown while a future is pending.
- GC tests cover a pending operation and collection after cancellation or
  completion. No hidden closure capture keeps an interpreter graph alive.
- Run the existing async/runtime suites and rebuild the bundled web runtime
  if shared runtime source changes, even though the public API starts native-only.

## Code references and decisions before implementation

- Public registration: `crates/sema/src/lib.rs`.
- Native callback ABI: `crates/sema-core/src/runtime/native.rs` and
  `crates/sema-core/src/value.rs`.
- Jobs and completion: `crates/sema-core/src/runtime/executor.rs` and
  `crates/sema-core/src/runtime/completion.rs`.
- Evaluation entry points: `crates/sema-eval/src/eval.rs`.
- Related conversion work: [earlier embedding API plan](2026-03-11-embedding-api-improvements.md).
  Its coroutine section is superseded; this proposal uses the current runtime.

Before implementation, settle whether three callbacks are acceptable for the
first API or whether a typed conversion helper should wrap them. Specify the
public worker error conversion and document the supported host runtime context.
An ambient Tokio caller must not be promised support until the full evaluation
path has been tested, including synchronous builtins called by the same script.
