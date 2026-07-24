# Poll Probes to Event-Driven Wakes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace runtime WebSocket polling with lossless watch-generation wakes, make timer-only `event/select` exact, and retain structural-timer polling only for VM-thread key/process readiness.

**Architecture:** WebSocket message receivers remain in their VM-thread `Rc<RefCell<...>>` slots; executor futures carry only cloned `tokio::sync::watch::Receiver<u64>` handles and plain results. The remaining `RuntimePoll` abstraction reports `Ready`, `Failed`, or `PendingAfter(Duration)`, and the shared continuation uses `WaitKind::Timer` rather than an off-thread sleep job.

**Tech Stack:** Rust 2021, Tokio `mpsc`/`oneshot`/`watch`, Sema `NativeOutcome`/`NativeContinuation`, unified runtime `WaitKind::External` and `WaitKind::Timer`, cargo-nextest, Jake.

## Global Constraints

- Do not add a new `WaitKind`, public Sema API, or scheduler protocol.
- Preserve synchronous top-level WebSocket, `event/select`, and key-read behavior.
- A cancelled `ws/recv` must leave the connection and installed receiver usable.
- Executor futures may carry only `Send + 'static` watch, timer, and plain result data; never `Value`, `Rc`, or `RefCell`.
- Any continuation that holds a `Value` must trace it; watch and receiver handles trace zero `Value` edges.
- A queued WebSocket message wins over `:timeout` at the deadline.
- `event/select` retains source-list priority when multiple sources are ready.
- Terminal input and `proc/*` readiness remain bounded VM-thread probes, documented as such.
- NativeFn closures must satisfy CORE-2 invariant I2: no strong capture that can transitively own a `Value` or `Env`.
- Follow red-green TDD: record the failing test and its expected failure before production edits.
- Follow the shipping invariant: no runtime source-tree paths, generated-only inputs, or developer-tool assumptions.

---

### Task 1: Structural-timer runtime probes

**Files:**
- Modify: `crates/sema-stdlib/src/io.rs:1392-1630`
- Modify: `crates/sema-stdlib/src/event.rs:64-190`
- Modify: `crates/sema/tests/vm_async_test.rs:1518-1600`

**Interfaces:**
- Consumes: `WaitKind::Timer(Duration)`, whose continuation resumes with `ResumeInput::Returned(nil)`.
- Produces:

```rust
pub(crate) enum RuntimePollResult {
    Ready(Value),
    Failed(String),
    PendingAfter(Duration),
}

pub(crate) trait RuntimePoll: Trace {
    fn poll(&mut self) -> RuntimePollResult;
}

pub(crate) fn await_runtime_until(
    probe: Box<dyn RuntimePoll>,
    started: Instant,
    timeout_ms: u64,
) -> NativeResult;
```

- Removes: `RUNTIME_POLL_COMPLETION_KIND`, `RuntimePollDecoder`, and the quarantined blocking inter-scan sleep.
- Later tasks remove every WebSocket implementation of `RuntimePoll`; after Task 3 only `SourcesProbe` and `KeyProbe` remain.

- [ ] **Step 1: Add failing unit tests for structural timer suspension**

In `io.rs`'s test module, define a zero-edge probe that requests 37 ms and assert that the helper returns a Timer suspension rather than External:

```rust
struct PendingProbe;

impl Trace for PendingProbe {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl RuntimePoll for PendingProbe {
    fn poll(&mut self) -> RuntimePollResult {
        RuntimePollResult::PendingAfter(Duration::from_millis(37))
    }
}

#[test]
fn runtime_poll_pending_uses_structural_timer() {
    let outcome = await_runtime_until(
        Box::new(PendingProbe),
        Instant::now(),
        1_000,
    )
    .expect("pending probe suspends");

    let NativeOutcome::Suspend(suspend) = outcome else {
        panic!("pending probe must suspend");
    };
    match suspend.wait {
        WaitKind::Timer(delay) => assert_eq!(delay, Duration::from_millis(37)),
        WaitKind::External(_) => panic!("poll probe must not occupy an executor worker"),
        _ => panic!("poll probe must use a structural timer"),
    }
}
```

Add a second test with `timeout_ms == 0` asserting immediate `NativeOutcome::Return(nil)` and no suspension.

- [ ] **Step 2: Run the tests and record RED**

Run:

```bash
cargo nextest run -p sema-stdlib runtime_poll_pending_uses_structural_timer
```

Expected: compile failure because `RuntimePollResult` does not exist and the current trait returns `Option<Result<Value, String>>`. This is the intended RED.

- [ ] **Step 3: Replace the off-thread sleep with `WaitKind::Timer`**

In `io.rs`, add `RuntimePollResult`, update `RuntimePoll`, and implement the shared step as:

```rust
fn runtime_poll_step(
    mut probe: Box<dyn RuntimePoll>,
    started: Instant,
    timeout_ms: u64,
) -> NativeResult {
    let requested_delay = match probe.poll() {
        RuntimePollResult::Ready(value) => return Ok(NativeOutcome::Return(value)),
        RuntimePollResult::Failed(message) => return Err(SemaError::eval(message)),
        RuntimePollResult::PendingAfter(delay) => delay,
    };

    let timeout = Duration::from_millis(timeout_ms);
    let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
        return Ok(NativeOutcome::Return(Value::nil()));
    };
    if remaining.is_zero() {
        return Ok(NativeOutcome::Return(Value::nil()));
    }

    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::Timer(requested_delay.min(remaining)),
        continuation: Box::new(RuntimePollContinuation {
            probe: Some(probe),
            started,
            timeout_ms,
        }),
    }))
}
```

Update `RuntimePollContinuation` to call this function after `Returned(_)`. Delete the completion-kind constant, decoder, quarantine bound, external operation, and imports that only served the sleep job.

Change `KeyProbe::poll()` to return `Ready`, `Failed`, or:

```rust
RuntimePollResult::PendingAfter(Duration::from_millis(5))
```

- [ ] **Step 4: Make `SourcesProbe` choose its next useful deadline**

Keep the existing ordered readiness scan first. If nothing is ready, compute the next check:

```rust
fn next_check_after(&self) -> Duration {
    let next_timer = self.sources.iter().filter_map(|source| {
        let map = source.as_map_ref()?;
        (map.get(&kw("type")) == Some(&kw("timer"))).then(|| {
            let ms = map
                .get(&kw("ms"))
                .and_then(|value| value.as_int())
                .unwrap_or(0)
                .max(0) as u128;
            Duration::from_millis(ms.saturating_sub(self.started.elapsed().as_millis()) as u64)
        })
    }).min();

    let has_vm_probe = self.sources.iter().any(|source| {
        source.as_map_ref()
            .and_then(|map| map.get(&kw("type")))
            .is_none_or(|kind| kind != &kw("timer"))
    });

    match (has_vm_probe, next_timer) {
        (false, Some(timer)) => timer,
        (true, Some(timer)) => timer.min(Duration::from_millis(5)),
        (true, None) | (false, None) => Duration::from_millis(5),
    }
}
```

Use an explicit match instead of `Option::is_none_or` if the workspace toolchain rejects it. `poll()` returns `PendingAfter(self.next_check_after())` after the ordered `ready` scan.

Add tests in `event.rs` proving that two timer sources at 100 ms and 25 ms produce a next delay no greater than 25 ms, while adding a bogus process source caps the delay at 5 ms. Allow elapsed-time drift by asserting ranges rather than exact `Instant` equality.

- [ ] **Step 5: Add integration coverage for timer-only selection**

In `vm_async_test.rs`, add:

```rust
#[test]
fn timer_only_event_select_preserves_earliest_source() {
    let out = eval(
        r#"
        (let ((event (event/select (list (time/tick 30) (time/tick 5)) 100)))
          (= (:source event) (time/tick 5)))
        "#,
    );
    assert_eq!(out, Value::bool(true));
}
```

Retain the existing cooperative sibling and read-key tests. Adjust the equality assertion if map identity makes `=` unsuitable: compare `(:ms (:source event))` with `5` instead.

- [ ] **Step 6: Run focused and package tests GREEN**

Run:

```bash
cargo nextest run -p sema-stdlib runtime_poll_pending_uses_structural_timer
cargo nextest run -p sema-stdlib event::tests
cargo nextest run -p sema-lang --test vm_async_test event_select
cargo nextest run -p sema-lang --test vm_async_test read_key_timeout
cargo fmt --check
```

Expected: all selected tests pass with no warnings or formatting drift.

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/sema-stdlib/src/io.rs crates/sema-stdlib/src/event.rs crates/sema/tests/vm_async_test.rs
git commit -m "refactor(runtime): drive VM-only probes with structural timers"
```

---

### Task 2: Event-driven client WebSocket waits

**Files:**
- Modify: `crates/sema-stdlib/src/ws.rs:31-90,131-220,331-590`
- Test: `crates/sema-stdlib/src/ws.rs` unit-test module
- Test: `crates/sema/tests/server_test.rs:1393-1565`

**Interfaces:**
- Consumes: `runtime_offload::external_io_async_try_with_continuation` and Tokio `oneshot`, `mpsc`, and `watch`.
- Produces:

```rust
struct WsConnection {
    cmd_tx: mpsc::UnboundedSender<WsFrame>,
    evt_rx: Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    evt_generation: watch::Receiver<u64>,
}

struct WsConnectContinuation { conn: Value }
struct WsRecvContinuation {
    evt_rx: Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    evt_generation: watch::Receiver<u64>,
    deadline: Option<Instant>,
}
```

- Removes: `WsConnectProbe`, `WsRecvProbe`, and all client `await_runtime_until` calls.
- The pump advances the watch generation after every queued `WsEvent` and once on exit.

- [ ] **Step 1: Add RED tests for lossless generation wakes and ownership**

Extract a private async helper with this exact contract:

```rust
async fn wait_for_ws_generation(
    mut generation: watch::Receiver<u64>,
    remaining: Option<Duration>,
) -> Result<(), String>;
```

Add Tokio unit tests that:

1. clone the receiver, call `borrow_and_update()`, advance the sender, and verify `changed()` resolves;
2. advance the sender before the wait future is first polled and verify the retained version still resolves;
3. drop a pending wait future, enqueue a `WsEvent`, and verify the original `mpsc::Receiver` still consumes it;
4. drop the watch sender and verify a pending wait resolves so the continuation can observe channel disconnection.

The cancellation ownership assertion must inspect the original receiver:

```rust
drop(wait_future);
evt_tx.try_send(WsEvent::Text("still-open".into())).unwrap();
assert!(matches!(evt_rx.try_recv(), Ok(WsEvent::Text(text)) if text == "still-open"));
```

- [ ] **Step 2: Run the tests and record RED**

Run:

```bash
cargo nextest run -p sema-stdlib ws_generation
cargo nextest run -p sema-stdlib dropping_ws_wait_preserves_receiver
```

Expected: compile failure because the watch helper and generation field do not exist.

- [ ] **Step 3: Add the connection generation channel and pump notifications**

Create `watch::channel(0_u64)` beside `evt_tx`/`evt_rx`. Pass the sender into `pump`, store the receiver in `WsConnection`, and return it from `handles_of`.

Use one helper for every pump event:

```rust
async fn publish_event(
    evt_tx: &mpsc::Sender<WsEvent>,
    generation: &watch::Sender<u64>,
    event: WsEvent,
) -> bool {
    if evt_tx.send(event).await.is_err() {
        return false;
    }
    generation.send_modify(|value| *value = value.wrapping_add(1));
    true
}
```

Replace every direct `evt_tx.send(...)` in `pump` with `publish_event`. Before `pump` returns, advance the generation once more; dropping the final sender then makes `changed()` return `Err` for any later waiter.

- [ ] **Step 4: Await the handshake oneshot directly**

Delete `WsConnectProbe`. In the runtime path, call `external_io_async_try_with_continuation` with the owned `ready_rx`. The executor future returns `Ok(())` for `Ok(Ok(()))` and preserves the existing error strings for `Ok(Err(message))` and a dropped sender.

`WsConnectContinuation` holds `conn: Value`, traces exactly one edge, returns that connection on `ResumeInput::Returned(_)`, propagates failure/cancellation, and rejects `ResumeInput::Runtime(_)` as an invariant error. The connection `Value` must never enter the Send future or decode closure.

- [ ] **Step 5: Replace client receive probes with re-arming watch waits**

Add `poll_ws_event` to perform the VM-thread `try_recv` and message-first deadline check:

```rust
fn poll_ws_event(
    evt_rx: &Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    deadline: Option<Instant>,
) -> Result<Option<Value>, SemaError>;
```

Its result is:

- `Ok(Some(value))` for a message or disconnect (`nil`);
- `Ok(Some(:timeout))` only after a final empty `try_recv` at or after the deadline;
- `Ok(None)` when a watch wait must be armed.

Add `suspend_ws_receive` which clones `evt_generation`, calls
`borrow_and_update()` before the final empty check, and uses
`external_io_async_try_with_continuation` to wait for a generation change or the
remaining deadline. Decode the plain `()` payload to `nil`; the supplied
`WsRecvContinuation` ignores that placeholder and calls `suspend_ws_receive`
again, so every wake rechecks the VM-owned receiver.

Both `ws_recv` and `ws_recv_timeout` call this path only inside a runtime quantum. Keep their existing synchronous fallbacks unchanged.

- [ ] **Step 6: Add trace and timeout-race tests**

Add unit tests asserting:

- `WsConnectContinuation` traces exactly its connection `Value`;
- `WsRecvContinuation` traces zero edges;
- a queued event at an expired deadline is returned instead of `:timeout`;
- an expired deadline with an empty queue returns `:timeout`;
- a coalesced wake with an empty queue produces another External suspension.

Update the ignored network round-trip tests only where comments name polling; do not make network-dependent tests required.

- [ ] **Step 7: Run Task 2 tests GREEN**

Run:

```bash
cargo nextest run -p sema-stdlib ws_generation
cargo nextest run -p sema-stdlib ws_connect_continuation
cargo nextest run -p sema-stdlib ws_recv_continuation
cargo nextest run -p sema-stdlib ws_timeout
cargo nextest run -p sema-lang --test server_test ws_recv
cargo fmt --check
cargo clippy -p sema-stdlib --all-targets -- -D warnings
```

Expected: all non-network tests pass; ignored network tests remain ignored unless explicitly selected.

- [ ] **Step 8: Commit Task 2**

```bash
git add crates/sema-stdlib/src/ws.rs crates/sema/tests/server_test.rs
git commit -m "refactor(ws): wake client operations from channel generations"
```

---

### Task 3: Event-driven server WebSocket receives

**Files:**
- Modify: `crates/sema-stdlib/src/server.rs:70-100,1054-1245,1353-1555`
- Modify: `crates/sema/tests/http_serve_concurrent_test.rs:212-260`
- Modify: `crates/sema/tests/http_serve_cancel_test.rs:113-200`

**Interfaces:**
- Consumes: Task 2's watch-generation ordering and `runtime_offload::external_io_async_try_with_continuation`.
- Produces:

```rust
ServerResponse::WebSocket {
    incoming_tx: mpsc::Sender<WsMsg>,
    incoming_generation: watch::Sender<u64>,
    outgoing_rx: mpsc::Receiver<WsMsg>,
}

struct ServerWsRecvContinuation {
    in_rx: Rc<RefCell<Option<mpsc::Receiver<WsMsg>>>>,
    incoming_generation: watch::Receiver<u64>,
}
```

- Removes: `ServerWsRecvProbe` and the cooperative server WebSocket call to `await_runtime_until`.

- [ ] **Step 1: Replace probe-unit tests with RED generation tests**

Delete the tests coupled to `ServerWsRecvProbe::poll`. Add tests proving:

- advancing `incoming_generation` wakes every receiver clone;
- dropping a pending generation future leaves `in_rx` installed;
- a queued text or binary `WsMsg` wins immediately;
- dropping `incoming_tx` plus the generation sender wakes the wait and resolves the next VM recheck to `nil`;
- `ServerWsRecvContinuation` traces zero `Value` edges.

Run:

```bash
cargo nextest run -p sema-stdlib server_ws_generation
```

Expected RED: the response field and continuation do not exist.

- [ ] **Step 2: Carry the generation sender through the axum bridge**

Add `incoming_generation` to `ServerResponse::WebSocket`, every constructor, and the response match that calls `bridge_websocket`.

Change `bridge_websocket` to advance the generation after every successful `incoming_tx.send(message).await`. On receive-loop exit, advance once more before dropping the last bridge-owned `incoming_tx`. This wakes pending evaluator receives for both graceful and abnormal disconnects.

The legacy serial handler may construct the watch channel but does not otherwise change its blocking receive behavior.

- [ ] **Step 3: Replace `ServerWsRecvProbe` with a watch continuation**

In `handle_ws_response_runtime`, create the watch channel beside `in_tx`/`in_rx`. Pass a sender clone to the bridge and retain a receiver clone in the runtime `recv_fn`.

The runtime receive path must:

1. clone the watch receiver and call `borrow_and_update()`;
2. check the VM-owned `in_rx` with `try_recv`;
3. return text, bytevector, or `nil` immediately when available/disconnected;
4. otherwise park on `changed()` through `external_io_async_try_with_continuation`;
5. re-enter the same helper from `ServerWsRecvContinuation` after every wake.

Give the `ws/close` closure a generation-sender clone. After clearing `in_rx`, advance the generation so a concurrent pending `ws/recv` observes closure even while the network bridge remains alive.

- [ ] **Step 4: Run server liveness and cancellation gates**

Run:

```bash
cargo nextest run -p sema-stdlib server_ws_generation
cargo nextest run -p sema-lang --test http_serve_concurrent_test
cargo nextest run -p sema-lang --test http_serve_cancel_test
cargo nextest run -p sema-lang --test server_async_test
cargo fmt --check
cargo clippy -p sema-stdlib --all-targets -- -D warnings
```

Expected: the idle-WebSocket head-of-line and server-root cancellation tests remain green, with no leaked handler task.

- [ ] **Step 5: Commit Task 3**

```bash
git add crates/sema-stdlib/src/server.rs crates/sema/tests/http_serve_concurrent_test.rs crates/sema/tests/http_serve_cancel_test.rs
git commit -m "refactor(server): wake websocket handlers from channel generations"
```

---

### Task 4: Purge guard, inventory, docs, and full gates

**Files:**
- Modify: `scripts/check-unified-runtime-legacy.sh`
- Modify: `docs/plans/evidence/unified-cooperative-runtime/runtime-match-map.tsv`
- Modify: `docs/deferred.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/plans/2026-07-18-poll-probes-event-driven-wakes.md`
- Modify: `docs/plans/2026-07-18-poll-probes-event-driven-wakes-implementation.md`

**Interfaces:**
- Consumes: Tasks 1-3 complete and reviewed.
- Produces: zero legacy WebSocket probes, an honest reconciled inventory, user-facing release notes, and a complete verification transcript in the task report.

- [ ] **Step 1: Plant-test the purge guard RED**

Extend `purged_pattern` with word-bounded entries for:

```text
WsConnectProbe|WsRecvProbe|ServerWsRecvProbe|RUNTIME_POLL_COMPLETION_KIND|RuntimePollDecoder
```

Plant one banned live-code declaration in a uniquely named scratch `.rs` file under `crates/sema-stdlib/src/`, run the guard, and record that it fails naming the file. Remove the scratch file and rerun to green. Do not use `git clean` for cleanup.

- [ ] **Step 2: Update documentation as current-state prose**

Add a concise `CHANGELOG.md` Unreleased bullet covering:

- WebSocket handshake/message/close wakes are event-driven;
- cancelling a receive preserves the connection;
- timer-only `event/select` parks once at the exact earliest deadline;
- stdin/process probes retain structural 5 ms timers because they cannot notify.

Update `docs/deferred.md` to remove polling residue claims that Tasks 1-3 resolved and explicitly retain the narrow stdin/process limitation. Change the design document status to `Implemented` only after every gate below passes. Do not use change-narration comments in Rust source.

- [ ] **Step 3: Reconcile the runtime inventory**

Run:

```bash
scripts/check-unified-runtime-inventory.sh --write-mapping
scripts/check-unified-runtime-inventory.sh --check
```

Reconcile new rows by exact `(path, text)` match first. Hand-classify only genuinely new or reworded rows against their adjacent ledger family. Confirm zero `UNREVIEWED` rows and describe every true reclassification in the task report.

- [ ] **Step 4: Run focused structural checks**

Run:

```bash
rg -n 'await_runtime_until|impl .*RuntimePoll|WsConnectProbe|WsRecvProbe|ServerWsRecvProbe' \
  crates/sema-stdlib/src/ws.rs crates/sema-stdlib/src/server.rs
scripts/check-unified-runtime-legacy.sh --check
scripts/check-unified-runtime-inventory.sh --check
```

Expected: `rg` finds none of the banned WebSocket probe/helper references; both guards pass.

- [ ] **Step 5: Run the full release-equivalent gate**

Run:

```bash
cargo nextest run --workspace
jake examples
jake smoke-bytecode
jake lint
jake docs-check
cargo check --target wasm32-unknown-unknown -p sema-wasm
```

Expected: every command exits zero. Any known flake must be isolation-reproduced and reported; do not label a failure pre-existing without that evidence.

- [ ] **Step 6: Commit Task 4**

```bash
git add scripts/check-unified-runtime-legacy.sh \
  docs/plans/evidence/unified-cooperative-runtime/runtime-match-map.tsv \
  docs/deferred.md CHANGELOG.md \
  docs/plans/2026-07-18-poll-probes-event-driven-wakes.md \
  docs/plans/2026-07-18-poll-probes-event-driven-wakes-implementation.md
git commit -m "docs(runtime): close event-driven wake slice"
```

- [ ] **Step 7: Generate the Slice 7 review package**

Resolve the implementation-plan commit as the Slice 7 code base, not `HEAD~1`:

```bash
slice7_base=$(git log --diff-filter=A -1 --format=%H -- \
  docs/plans/2026-07-18-poll-probes-event-driven-wakes-implementation.md)
/Users/helge/.codex/plugins/cache/claude-plugins-official/superpowers/6.1.1/skills/subagent-driven-development/scripts/review-package "$slice7_base" HEAD
```

Dispatch the task reviewer with this plan, every task report, the printed review-package path, and the Global Constraints above. Critical and Important findings must be fixed and re-reviewed before Slice 7 is marked complete.
