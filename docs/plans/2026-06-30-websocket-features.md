# WebSocket Features in Sema — Investigation & Plan

Tracking issue: [#49](https://github.com/HelgeSverre/sema/issues/49)
Status: **Phases 1–2 implemented** — see `crates/sema-stdlib/src/ws.rs`, the
`with-open`/`ws/listen` prelude macros, and `test_websocket_*` in `server_test.rs`.
Phases 3–4 below remain proposed.

## TL;DR

- **Server-side WebSockets already ship.** `(http/router [[:ws "/path" handler]])` upgrades a
  route and hands the handler a `conn` map of closures `{:send :recv :close}` (text frames only).
  See `crates/sema-stdlib/src/server.rs` (`http/websocket`, `handle_ws_response`, `bridge_websocket`).
- **The real gap in #49 is the client.** There is no `ws/connect`. That is what this plan builds.
- **The async machinery to do it well already exists.** The HTTP client's offload pattern
  (`IoHandle` poller + abort hook, `set_yield_signal(AwaitIo)`, the shared tokio runtime, and the
  cooperative scheduler) is a drop-in template. No changes to core async infrastructure are required.
- **Recommended shape:** represent a connection as a first-class **stream-handle value** and expose a
  message-oriented `ws/*` namespace (`ws/connect`, `ws/send`, `ws/recv`, `ws/close`, ...). The
  blocking/pull model is the MVP; an evented (`:on-message`) API is layered on top later as a prelude
  macro, not a second primitive. Reuse `with-stream` (aliased to `with-open`) for RAII cleanup.

## 1. What exists today

### 1.1 Server side (already implemented)

| Piece | Location |
|-------|----------|
| `http/websocket` marker factory | `crates/sema-stdlib/src/server.rs:225` |
| WS route match in router (`:ws`) | `server.rs:554`, `server.rs:670` |
| `handle_ws_response` (builds `conn` map of `ws/send`,`ws/recv`,`ws/close` closures) | `server.rs:1113` |
| `bridge_websocket` (axum socket ↔ evaluator mpsc channels) | `server.rs:969` |
| Integration tests | `crates/sema/tests/server_test.rs:1248`, `:1291`; `integration_test.rs:12419` |

Canonical server usage (from the test suite):

```scheme
(http/serve
  (http/router
    [[:ws "/chat" (fn (conn)
      (let loop ()
        (let ((msg ((:recv conn))))          ; blocks; nil on close
          (when msg
            ((:send conn) (string-append "re:" msg))
            (loop)))))]])
  {:port 19900})
```

Notes / limitations of the current server path:
- **Text frames only** — the bridge channels are `tokio::mpsc::Sender<String>` (`server.rs:40-42`,
  `:982`, `:999`). Binary frames are dropped.
- The connection is a **map of closures** (`{:send :recv :close}`), invoked as `((:recv conn))`.
  This is ergonomic-adjacent but differs from the `(ws/send conn …)` namespace style #49 prefers.
- No ping/pong control, no close-code/reason surfaced to Sema, no per-message back-pressure knob
  beyond the fixed 256-capacity channel.

### 1.2 Async / I/O infrastructure (ready to reuse)

The HTTP client demonstrates the exact pattern a WS client needs (`crates/sema-stdlib/src/http.rs`):

- **Dual path.** Top level → `tokio::Runtime::block_on` (blocks a runtime thread, not the VM).
  Inside `async/spawn` → `in_async_context()` is true, so the op offloads work to the
  process-wide `stdlib_shared_rt()` (`crates/sema-stdlib/src/async_rt.rs`) and **yields**.
- **Yield + resume.** `set_yield_signal(YieldReason::AwaitIo(handle))` parks the task; the scheduler
  polls `handle.poll()` (non-blocking) and resumes with the cached value. Natives never re-run on
  resume — they pick up `take_resume_value()`. See `crates/sema-core/src/async_signal.rs`,
  `crates/sema-vm/src/scheduler.rs`.
- **Wakeups.** The offloaded tokio task calls `sema_core::notify_io_complete()` to unpark the VM.
- **True cancellation.** `IoHandle::with_abort(poll, abort)` lets `async/timeout` / `async/cancel`
  abort the in-flight tokio task (`http.rs:225-239`).
- **Channels** already exist in Sema (`channel/new|send|recv|try-recv|close`,
  `crates/sema-stdlib/src/async_ops.rs:488+`) — useful if we expose a channel-based recv stream.

### 1.3 Value representation & resource cleanup

- `Value` carries typed runtime handles as dedicated variants (`Channel`, `AsyncPromise`,
  `Stream(Rc<StreamBox>)`), plus a generic escape hatch `NativeFn.payload: Rc<dyn Any>`
  (`crates/sema-core/src/value.rs`).
- `SemaStream` is the intended trait for "opaque Rust resource behind `Rc` with explicit `close()`"
  (`value.rs:379`). `StreamBox` exposes `as_any()` for downcasting, and `Drop` only drops the `Rc`
  (close is explicit + idempotent).
- `with-stream` (prelude macro, `crates/sema-eval/src/prelude.rs:68`) already gives RAII:
  it `stream/close`s on both the success and error paths. There is **no** `with-open` yet.

### 1.4 Dependencies & sandbox

- Present: `tokio` (full), `reqwest`, `axum` (with `ws`), `futures`, `tokio-stream`.
  `tungstenite 0.28` is a **dev-dependency** of `crates/sema` (tests only).
- **Missing for an async client:** `tokio-tungstenite` (not in the tree).
- Network is gated by `register_fn_gated(env, sandbox, Caps::NETWORK, …)`
  (`crates/sema-stdlib/src/lib.rs:106`). All `http/*` use it; `ws/*` must too.
- Net features are `cfg(not(target_arch = "wasm32"))` — the client is **native-only** for now.

## 2. The design question from #49

> callbacks/event-loop **vs** a Lisp-friendly channel/stream abstraction.

**Recommendation: make the channel/pull model the primitive; layer the evented API on top.**

Rationale:
1. Sema's scheduler is **cooperative and single-threaded**. A callback can only fire when Sema code
   yields, so an "event loop" is really a recv-loop in disguise. Making the loop explicit (pull) is
   the honest primitive; the callback form is sugar that spawns that loop for you.
2. The pull model composes with everything Sema already has: `async/spawn`, `async/all`, `channel/*`,
   `match`, `with-stream`. The evented model does not compose — it inverts control.
3. It mirrors the **already-shipped server side** (`((:recv conn))` loop), so client and server read
   the same way.

So the canonical client program is:

```scheme
(with-open (sock (ws/connect "wss://echo.websocket.events"))
  (ws/send sock {:json {:type "ping"}})
  (match (ws/recv sock)
    {:text msg}   (println msg)
    {:binary buf} (handle-bytes buf)
    {:close info} :done))
```

…and the evented form (Phase 2) is a macro that expands to an `async/spawn` recv-loop:

```scheme
(ws/listen (ws/connect "wss://…")
  {:on-open    (fn (sock) …)
   :on-message (fn (sock msg) …)
   :on-close   (fn (sock code reason) …)
   :on-error   (fn (err) …)})
```

## 3. Connection value: choosing the representation

Three options, ranked:

1. **Stream-handle (recommended).** Implement `WsConnection: SemaStream`, return it as
   `Value::Stream`. Benefits:
   - **Zero new core variants** — no NaN-box tag, `Drop`, `type_name`, equality, or serializer churn.
     (Live sockets are never serialized; `Stream`/`Channel` are already runtime-only values.)
   - **RAII for free:** `with-stream` already closes streams; we alias `with-open` → `with-stream`,
     so it works uniformly for files *and* sockets.
   - `ws/*` functions downcast `StreamBox::as_any()` → `&WsConnection` to reach the typed API.
   - The byte-stream methods (`stream/read`/`write`) are simply left unsupported (return a clear
     "use ws/recv on a websocket" error) — `ws/*` is the message-oriented surface.
2. **Map-of-closures** (what the server does). Fastest to write, zero core changes, but `((:send c) …)`
   is clunky and #49 explicitly wants `(ws/send c …)`. Good fallback if we want a one-day MVP.
3. **Dedicated `Value::WebSocket(Rc<WsConn>)` variant.** Cleanest typing, matches the repo's taste of
   giving important resources their own variant — but touches NaN-boxing, `Drop`, `ValueView`,
   `type_name`, equality, and the (reject) path in the bytecode serializer. Defer unless `ws/*` grows
   enough that stream-aliasing feels like a hack.

**Decision:** go with **(1) stream-handle** for the MVP; revisit (3) only if the surface area justifies it.

## 4. Implementation sketch (client)

New file `crates/sema-stdlib/src/ws.rs`, registered from `lib.rs` like the other modules.

```rust
struct WsConnection {
    // evaluator <- network: messages from the server
    incoming_rx: RefCell<tokio::sync::mpsc::Receiver<WsFrame>>,
    // evaluator -> network: messages to the server
    outgoing_tx: tokio::sync::mpsc::Sender<WsFrame>,
    abort: tokio::task::AbortHandle,   // kill the pump task on close/cancel
    closed: Cell<bool>,
}
enum WsFrame { Text(String), Binary(Vec<u8>), Close { code: u16, reason: String } }
impl SemaStream for WsConnection { /* close() drops tx + aborts pump; read/write -> error */ }
```

`ws/connect`:
1. Validate URL + parse options map (`:headers`, `:subprotocols`, `:timeout`).
2. `sandbox.check(Caps::NETWORK, "ws/connect")`.
3. Spawn a **pump task** on `stdlib_shared_rt()`: `tokio_tungstenite::connect_async(req)`, then
   `select!` between (a) `incoming` ws stream → `incoming_tx`, calling `notify_io_complete()` on each
   frame, and (b) `outgoing_rx` → ws sink. This is the same bridge shape as `bridge_websocket`.
4. Connection establishment is itself async: at top level `block_on` the handshake; inside an async
   task, yield `AwaitIo` until the handshake oneshot resolves, then return the `Value::Stream`.

`ws/recv` (typed): top level → `incoming_rx.blocking_recv()`; async → yield `AwaitIo` whose poller does
`incoming_rx.try_recv()`. Maps `WsFrame` → `{:text s}` / `{:binary bv}` / `{:close {:code n :reason s}}`,
and `nil` once the stream is drained + closed.

`ws/send`: accepts a string (`:text`), a bytevector (`:binary`), or `{:json v}` (encodes via
`value_to_json_lossy` like `http/post`). Top level → `outgoing_tx.blocking_send`; async → non-blocking
`try_send`, yielding if the channel is full (back-pressure).

`ws/close`: drop `outgoing_tx`, `abort()` the pump, set `closed`; idempotent. Reached automatically by
`with-open`/`with-stream`.

Other surface: `ws/connected?`, `ws/recv-timeout` (ms), `ws/ping` (Phase 2).

`with-open` macro: add to `crates/sema-eval/src/prelude.rs` as an alias of `with-stream` (identical
expansion). Keeps one cleanup path for files and sockets.

### Dependency change

`crates/sema-stdlib/Cargo.toml` (under the `cfg(not(wasm32))` block):
```toml
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
```
Use **rustls** to match `reqwest`'s TLS stack and avoid pulling a second (native-tls) stack for `wss://`.

## 5. Making it comprehensive & practical

Beyond the bare connect/send/recv MVP, these are what make it usable in anger:

- **`wss://` / TLS** — first-class, via the rustls feature above. (MVP, not optional.)
- **Binary frames** — both directions, client and server (extend the server's `String` bridge to a
  `WsFrame` enum; surface `{:binary bv}`).
- **JSON convenience** — `(ws/send sock {:json v})` and an optional `(ws/recv-json sock)` that decodes.
- **Connect options** — `:headers` (auth bearer tokens), `:subprotocols`, `:timeout`,
  `:max-message-size`.
- **Timeouts & keepalive** — `ws/recv-timeout`; periodic ping (tungstenite auto-pongs incoming pings),
  optional `:ping-interval`.
- **Close semantics** — surface close code + reason; send a proper close frame on `ws/close`.
- **Back-pressure** — bounded outgoing channel; document the capacity / make it an option.
- **Cancellation** — wire `IoHandle::with_abort` so `async/timeout`/`async/cancel` truly kill an
  in-flight connect or recv (the pump's `AbortHandle`).
- **Errors with hints** — connection refused, handshake/HTTP-status failure, abnormal close, send-after-
  close → `SemaError::eval(...).with_hint(...)`.
- **Sandbox** — `Caps::NETWORK` gating, and honor any host allowlist the sandbox grows.
- **Reconnect helper** — Phase 2 prelude/stdlib helper with exponential backoff (mirrors the LLM
  client's retry ethos).
- **Evented sugar** — `ws/listen` macro (Phase 2) for the `:on-message` style from the issue.
- **Server parity** — migrate the server `conn` toward the same `ws/*` verbs (keep `:send/:recv/:close`
  closures for back-compat) and add binary frames there too.

## 6. Testing strategy

Follow the repo convention (network tests `#[ignore]`, deterministic tests always-on):

1. **Round-trip, keyless, in-process** — start the *existing* Sema `http/serve` WS echo server on a
   port and connect the new `ws/connect` client to it; assert text + binary round-trips and clean
   close. (Inverse of today's `tungstenite::connect → Sema server` tests.) Marked `#[ignore]` like the
   other server/http tests.
2. **Error-path eval tests** (no network) — bad URL, send-after-close, wrong arg types, `ws/recv` on a
   non-socket stream → assert `SemaError` messages. Goes in `eval_test.rs`.
3. **Async/concurrency** — in `vm_async_test.rs`: two `async/spawn` tasks each `ws/recv` on separate
   connections, plus `async/timeout` cancelling a hung `ws/recv` (proves the abort hook).
4. **Lint/docs gates** — `make lint`, and a docs page so `make docs-check` passes.

## 7. Phasing

- **Phase 0 — decision.** ✅ Confirmed §2 (pull-first) and §3 (stream-handle).
- **Phase 1 — client MVP.** ✅ Done. `ws/connect`, `ws/send` (text/binary/JSON), `ws/recv` (typed),
  `ws/close`, `ws/connected?`; `with-open`; `wss://`/TLS via rustls; `Caps::NETWORK`; dual
  top-level/async paths; ignored round-trip test + no-network error-path tests; docs in
  `web-server.md`. (Playground example deferred — native-only.)
- **Phase 2 — comprehensive client.** ✅ Done. Explicit `{:text/:binary/:json}` framing;
  connect opts (`:headers`, `:subprotocols`, `:timeout`, `:retries`, `:retry-backoff-ms` with
  exponential backoff); `ws/recv-timeout` (→ `:timeout`); `ws/ping`; the `ws/listen` evented
  macro. Tests in `server_test.rs` (`test_websocket_client_options_and_framing`,
  `test_websocket_listen`); docs in `web-server.md`.
- **Phase 3 — server parity.** Binary frames server-side, unify `conn` under `ws/*`, close codes,
  ping/pong.
- **Phase 4 — reach (optional).** WASM/browser client via `web-sys` WebSocket; LSP signatures/docs;
  more playground examples.

## 8. Risks & open questions

- **Single shared runtime saturation.** Long-lived WS pump tasks live on `stdlib_shared_rt()` alongside
  HTTP offloads. Many idle sockets are cheap (parked on `select!`), but worth a note/limit.
- **`stream/read` on a socket.** Decision above is to error rather than silently bridge bytes; confirm
  that's the desired ergonomics vs. making a WS genuinely behave as a byte stream.
- **`with-open` naming.** Alias of `with-stream` now; if we later add non-stream closeables, promote it
  to a protocol-dispatching macro (generic `close`). Recorded as a deferred concern.
- **Server `conn` migration** is a (small) breaking change if we ever drop the closure map; plan keeps
  both during Phase 3.
- **wasm:** client is native-only; the playground stays without WS unless Phase 4 is done.
