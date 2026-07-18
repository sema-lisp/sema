# Poll Probes to Event-Driven Wakes

**Status:** Approved 2026-07-18

## Goal

Remove fixed-interval polling from runtime WebSocket operations, make timer-only
`event/select` waits exact, and retain bounded polling only for readiness sources
that cannot notify the runtime. Public Sema behavior stays unchanged.

## Current behavior

`crates/sema-stdlib/src/io.rs` implements `await_runtime_until` by checking a
`RuntimePoll`, scheduling a quarantined blocking job that sleeps for 5 ms, then
checking the probe again. The helper serves unrelated operations:

- client `ws/connect`, `ws/recv`, and `ws/recv-timeout`;
- server-side `ws/recv`;
- `event/select` over key, process, and timer sources;
- `io/read-key-timeout`.

The helper is cooperative—the VM thread does not block—but every idle wait can
submit about 200 executor jobs per second. A WebSocket message can also wait for
the next scan even though Tokio channels already provide wake notifications.

The sources do not have one common readiness model. WebSocket handshakes and
message queues can notify immediately. Timer sources have exact deadlines.
Terminal input and `proc/*` readiness live behind synchronous VM-thread checks
and have no runtime notification interface.

## Decision

Use each source's strongest existing wake mechanism. Do not add a runtime-wide
wait-set or redesign terminal and process registries in this slice.

| Source | Runtime wait |
|---|---|
| WebSocket handshake | async External wait on the existing oneshot |
| Client WebSocket message/close | async External wait on a versioned Tokio watch signal |
| Server WebSocket message/close | async External wait on a versioned Tokio watch signal |
| Timer-only `event/select` | one structural `WaitKind::Timer` |
| Mixed `event/select` | structural timer until the next timer deadline or 5 ms probe, whichever is earlier |
| `io/read-key-timeout` | structural 5 ms timer between VM-thread readiness checks |

This keeps `WaitKind::External` for real external notifications and uses
`WaitKind::Timer` for deadlines and unavoidable probe intervals. No new
`WaitKind`, public API, or scheduler protocol is required.

## WebSocket notification design

### Preserve receiver ownership

The client and server receivers remain in their VM-thread-owned
`Rc<RefCell<...>>` slots. An async wait must not move a receiver into an
executor future: cancelling that future would drop or strand the receive half
and poison the connection.

Instead, each incoming message queue has a versioned Tokio watch signal. The
network pump enqueues the message, then increments the watched generation. On
every pump exit it advances or closes the watch channel so every waiter can
observe channel disconnection.

```text
network pump -> enqueue message -> advance generation -> External completion
                                                |
                                                v
                                      VM continuation runs
                                                |
                                                v
                                           try_recv()
```

The runtime native clones a watch receiver, marks the current generation as
seen, then checks `try_recv()`. When the queue is empty, it parks on
`watch::Receiver::changed()`. The continuation checks `try_recv()` again after
waking. If generations were coalesced or another pending receive consumed the
message, the continuation rearms without blocking the VM thread.

The ordering closes the empty-check/park race: a message queued before the
generation snapshot is visible to `try_recv()`, while a generation advanced
after the snapshot makes `changed()` ready. Unlike a bare
`Notify::notify_waiters()`, the watch version is retained when the sender wins
the race before the async waiter begins polling. Each pending receive owns a
watch-receiver clone, so one generation wakes every waiter without moving the
message receiver off the VM thread.

### Client connection handshake

`ws/connect` awaits its existing oneshot directly through an async External
operation. The connection `Value` stays in a traced continuation and never
crosses to the executor. Cancelling the handshake drops the pending connection,
which closes its command channel and stops the pump, matching current behavior.

### Receive timeouts

`ws/recv-timeout` races the watch change against a Tokio timer. A timeout wake
performs one final `try_recv()` before returning `:timeout`, preserving the
current rule that an already-queued message wins at the deadline.

### Cancellation and close

Cancelling a pending receive drops only its watch-receiver clone. The receiver
remains installed, so a later `ws/recv` can consume the next message.

Closing or losing the socket wakes all pending receive waiters. They recheck the
channel and resolve to `nil`. No waiter remains parked waiting for a notification
that can no longer be produced.

Concurrent receives retain the current first-consumer behavior: one waiter
consumes each message; other awakened waiters recheck and rearm.

## Poll-only source design

### Replace executor sleeps with structural timers

`RuntimePoll` remains only for VM-thread readiness checks. Its pending result
states when the next check is useful rather than relying on a global off-thread
sleep:

```rust
enum RuntimePollResult {
    Ready(Value),
    Failed(String),
    PendingAfter(Duration),
}
```

The shared continuation translates `PendingAfter` into `WaitKind::Timer` and
rechecks the probe when the timer fires. The quarantined blocking sleep,
completion decoder, completion-kind tag, and WebSocket probe implementations
are removed.

The concrete implementation may use an equivalent internal type shape, but the
three states and their semantics are fixed.

### `event/select`

`SourcesProbe` computes its next useful check from the source set:

- With only timer sources, wait until the earliest timer. This produces one
  structural timer wait rather than repeated 5 ms scans.
- With key or process sources, check those sources at most every 5 ms. If a
  timer or explicit timeout expires sooner, use that earlier deadline.
- On each wake, preserve source-list order when choosing the first ready source.

Example:

```scheme
(event/select (list (time/tick 100) (time/tick 25)))
```

This parks once for approximately 25 ms. It does not perform five intermediate
scans.

This mixed case retains bounded polling because key and process sources cannot
signal the runtime:

```scheme
(event/select
  (list {:type :key}
        {:type :proc :handle child}
        (time/tick 100))
  1000)
```

The helper rechecks key/process readiness every 5 ms and uses the earlier timer
deadline when less than 5 ms remains.

### `io/read-key-timeout`

`io/read-key-timeout` keeps its synchronous stdin readiness check and requests
another structural timer wake after 5 ms. This retains its cooperative behavior
without occupying executor workers or routing empty sleep completions through
the runtime inbox.

## Error and race semantics

- A WebSocket message queued at the timeout boundary wins over `:timeout`.
- A disconnected WebSocket resolves `ws/recv` to `nil`.
- A failed handshake retains its current domain error.
- A cancelled receive does not close or tombstone the connection.
- A cancelled handshake drops the incomplete connection and its pump.
- Spurious or coalesced readiness generations cause a recheck and rearm, not an
  error.
- A closed pump wakes every pending receive before no further notification can
  be produced.
- `event/select` retains source-list priority when multiple sources are ready.
- Probe cancellation follows existing sticky runtime cancellation semantics.

## GC and ownership invariants

- No `NativeFn` closure may strongly capture state that transitively owns a
  `Value` or `Env` (CORE-2 invariant I2).
- A continuation that holds the pending client connection `Value` must trace it.
- Receiver cells and watch handles contain no `Value` and report no GC
  edges.
- Executor futures carry only `Send + 'static` watch, timer, and plain
  result data. They never carry `Value`, `Rc`, or `RefCell`.
- Cancellation cannot drop a live installed receiver.

## Verification

Implementation follows red-green TDD. Focused tests must prove:

1. Client and server WebSocket receives wake from watch changes without a
   periodic runtime timer.
2. Cancelling a pending receive leaves the connection usable by a later
   receive.
3. Pump shutdown wakes all pending receivers and they resolve to `nil`.
4. A message queued at the timeout boundary wins over `:timeout`.
5. Spurious/coalesced readiness generations rearm safely.
6. The client handshake completes from its oneshot and cancellation stops the
   incomplete connection.
7. Timer-only `event/select` requests the exact earliest deadline.
8. Mixed key/process/timer selection keeps source priority and chooses the
   earlier of the probe interval, source timer, and explicit timeout.
9. `io/read-key-timeout` still yields to sibling tasks.
10. Traced continuations report exactly their live `Value` edges.

Structural guards must show:

- `ws.rs` and the cooperative server WebSocket path no longer call
  `await_runtime_until` or implement WebSocket `RuntimePoll` probes;
- the old quarantined 5 ms runtime-poll job and decoder are absent;
- poll-only source comments name why polling remains.

Required regression gates:

```bash
cargo nextest run -p sema-lang --test vm_async_test
cargo nextest run -p sema-lang --test integration_test
cargo nextest run --workspace
jake examples
jake smoke-bytecode
jake lint
jake docs-check
scripts/check-unified-runtime-legacy.sh --check
scripts/check-unified-runtime-inventory.sh --check
```

## Out of scope

- A general runtime wait-set or `select` primitive.
- New stdin readiness threads or platform-specific event-loop integration.
- Process-registry subscriptions or per-handle wakers.
- Changes to synchronous top-level WebSocket, `event/select`, or key-read
  behavior beyond comments needed to describe the current path.
- Public Sema API or return-value changes.
- WebAssembly support for native-only WebSocket and terminal paths.

Those notification sources can replace the remaining bounded probes in a later
slice without changing the interfaces established here.
