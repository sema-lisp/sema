//! WebSocket client (`ws/*`). Native-only; gated on `Caps::NETWORK`.
//!
//! A connection is a `Value::Stream` wrapping [`WsConnection`] (a `SemaStream`),
//! so `with-open`/`stream/close` give RAII cleanup for free. The byte-stream
//! `read`/`write` methods are intentionally unsupported — the message-oriented
//! `ws/send`/`ws/recv` surface is the API.
//!
//! Mirrors the HTTP client's offload model (`http.rs`): a long-lived **pump**
//! task runs on the shared tokio runtime, bridging the socket to two channels —
//! an *unbounded* outgoing command channel and a *bounded* incoming event
//! channel. Top-level ops block the VM thread; ops inside an `async/spawn` task
//! suspend on structural external waits so siblings run during a receive or
//! handshake.

#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    CompletionKind, NativeCallContext, NativeContinuation, NativeOutcome, NativeResult,
    ResumeInput, Trace,
};
use sema_core::{check_arity, Caps, SemaError, SemaStream, StreamBox, Value};

use crate::register_fn;

const WS_COMPLETION_KIND: u64 = 0x7773_0000; // "ws\0\0"

/// Retains the connection on the VM thread while its handshake runs on the I/O
/// executor. Dropping this continuation closes the command channel and stops the
/// incomplete pump.
struct WsConnectContinuation {
    conn: Value,
    abort_pump: Option<sema_core::AbortHook>,
}

impl Trace for WsConnectContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.conn));
        true
    }
}

impl NativeContinuation for WsConnectContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(_) => {
                self.abort_pump.take();
                Ok(NativeOutcome::Return(self.conn.clone()))
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "ws/connect was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "ws/connect continuation received an unexpected runtime response",
            )),
        }
    }
}

impl Drop for WsConnectContinuation {
    fn drop(&mut self) {
        if let Some(abort_pump) = self.abort_pump.take() {
            abort_pump();
        }
    }
}

/// Rechecks the VM-owned event receiver after every generation or timer wake.
/// The watch handle has no `Value` edges and dropping an in-flight wait leaves
/// the installed message receiver intact.
struct WsRecvContinuation {
    evt_rx: Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    evt_generation: watch::Receiver<u64>,
    deadline: Option<Instant>,
}

impl Trace for WsRecvContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for WsRecvContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(_) => {
                suspend_ws_receive(self.evt_rx, self.evt_generation, self.deadline)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "ws/recv was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "ws/recv continuation received an unexpected runtime response",
            )),
        }
    }
}

/// Capacity of the incoming-event channel. Bounded so a slow Sema consumer
/// applies back-pressure to the network read side instead of buffering without
/// limit. Outgoing commands use an *unbounded* channel so `ws/send` never blocks.
const EVENT_CAP: usize = 1024;

/// Outgoing command from the evaluator to the pump task.
enum WsFrame {
    Text(String),
    Binary(Vec<u8>),
    /// Ping with an arbitrary payload; the server replies with a matching Pong.
    Ping(Vec<u8>),
    /// Graceful close: pump sends a Close frame and exits.
    Close,
}

/// Options parsed from the `ws/connect` opts map (all optional).
#[derive(Default)]
struct ConnectOpts {
    /// Extra HTTP headers on the upgrade request (e.g. `Authorization`).
    headers: Vec<(String, String)>,
    /// `Sec-WebSocket-Protocol` values offered to the server.
    subprotocols: Vec<String>,
    /// Handshake timeout in milliseconds (`None` = wait indefinitely).
    timeout_ms: Option<u64>,
    /// How many times to retry a failed handshake before giving up.
    retries: u32,
    /// Base backoff between handshake retries; doubles each attempt (capped).
    backoff_ms: u64,
}

/// Incoming event from the pump task to the evaluator.
enum WsEvent {
    Text(String),
    Binary(Vec<u8>),
    Close { code: u16, reason: String },
    Error(String),
}

/// A live client WebSocket. All I/O goes through the channels to the pump task;
/// the `SemaStream` byte methods are deliberately unsupported.
struct WsConnection {
    cmd_tx: mpsc::UnboundedSender<WsFrame>,
    evt_rx: Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    evt_generation: watch::Receiver<u64>,
}

impl std::fmt::Debug for WsConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<websocket>")
    }
}

impl SemaStream for WsConnection {
    fn read(&self, _buf: &mut [u8]) -> Result<usize, SemaError> {
        Err(SemaError::eval("stream/read: not supported on a websocket")
            .with_hint("use (ws/recv conn) to receive messages"))
    }

    fn write(&self, _data: &[u8]) -> Result<usize, SemaError> {
        Err(
            SemaError::eval("stream/write: not supported on a websocket")
                .with_hint("use (ws/send conn msg) to send messages"),
        )
    }

    /// Best-effort graceful close: ask the pump to send a Close frame and exit.
    /// Idempotent — `StreamBox` guards against a double close. Dropping the
    /// connection value (last `cmd_tx`) also stops the pump, so cleanup happens
    /// even without an explicit close.
    fn close(&self) -> Result<(), SemaError> {
        let _ = self.cmd_tx.send(WsFrame::Close);
        Ok(())
    }

    /// The pump drops its `cmd_rx` when it exits (server close / error), which
    /// flips the sender to closed — so this tracks whether the socket is live.
    fn is_writable(&self) -> bool {
        !self.cmd_tx.is_closed()
    }

    fn is_readable(&self) -> bool {
        !self.cmd_tx.is_closed()
    }

    fn stream_type(&self) -> &'static str {
        "websocket"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Resolve an argument to a websocket `StreamBox`, rejecting non-stream and
/// non-websocket streams with an actionable error.
fn ws_conn(args: &[Value], fname: &str, idx: usize) -> Result<Rc<StreamBox>, SemaError> {
    let arg = args
        .get(idx)
        .ok_or_else(|| SemaError::arity(fname, format!("{}", idx + 1), args.len()))?;
    let sb = arg.as_stream_rc().ok_or_else(|| {
        SemaError::type_error("websocket", arg.type_name()).with_hint(format!(
            "{fname} expects a websocket connection as argument {}",
            idx + 1
        ))
    })?;
    if sb
        .borrow_inner()
        .as_any()
        .downcast_ref::<WsConnection>()
        .is_none()
    {
        return Err(
            SemaError::type_error("websocket", sb.stream_type()).with_hint(format!(
                "{fname} expects a websocket, got a {} stream",
                sb.stream_type()
            )),
        );
    }
    Ok(sb)
}

/// Clone the channel handles out of a websocket `StreamBox`, releasing the inner
/// borrow before the caller does any (possibly blocking) channel I/O.
fn handles_of(
    sb: &StreamBox,
) -> (
    mpsc::UnboundedSender<WsFrame>,
    Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    watch::Receiver<u64>,
) {
    let inner = sb.borrow_inner();
    let conn = inner.as_any().downcast_ref::<WsConnection>().unwrap();
    (
        conn.cmd_tx.clone(),
        conn.evt_rx.clone(),
        conn.evt_generation.clone(),
    )
}

/// Encode a value as JSON text for a frame.
fn json_text(v: &Value) -> Result<String, SemaError> {
    let json = sema_core::value_to_json_lossy(v);
    serde_json::to_string(&json).map_err(|e| SemaError::eval(format!("ws/send: json encode: {e}")))
}

/// Translate a Sema value into an outgoing frame.
///
/// Shorthands: a string → text frame, a bytevector → binary frame, a plain map →
/// JSON text. The framing can also be made explicit with a single-key tagged
/// map, mirroring what `ws/recv` returns: `{:text s}`, `{:binary bv}`, or
/// `{:json v}` (encodes `v` as JSON, so `{:json {…}}` sends the inner value).
fn value_to_frame(v: &Value) -> Result<WsFrame, SemaError> {
    if let Some(s) = v.as_str() {
        return Ok(WsFrame::Text(s.to_string()));
    }
    if let Some(bv) = v.as_bytevector_rc() {
        return Ok(WsFrame::Binary(bv.to_vec()));
    }
    if let Some(m) = v.as_map_rc() {
        // Explicit tagged framing takes precedence over the plain-map shorthand.
        if let Some(inner) = m.get(&Value::keyword("json")) {
            return Ok(WsFrame::Text(json_text(inner)?));
        }
        if let Some(inner) = m.get(&Value::keyword("text")) {
            let s = inner
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", inner.type_name()))
                .map_err(|e| e.with_hint("ws/send {:text …} expects a string"))?;
            return Ok(WsFrame::Text(s.to_string()));
        }
        if let Some(inner) = m.get(&Value::keyword("binary")) {
            let bv = inner
                .as_bytevector_rc()
                .ok_or_else(|| SemaError::type_error("bytevector", inner.type_name()))
                .map_err(|e| e.with_hint("ws/send {:binary …} expects a bytevector"))?;
            return Ok(WsFrame::Binary(bv.to_vec()));
        }
        // Plain map → JSON text.
        return Ok(WsFrame::Text(json_text(v)?));
    }
    Err(SemaError::type_error("string, bytevector, or map", v.type_name()).with_hint(
        "ws/send accepts a string (text), a bytevector (binary), or a map ({:json/:text/:binary …} or a plain map sent as JSON)",
    ))
}

/// Decode an incoming event into the Sema value `ws/recv` returns. `None` (the
/// channel drained and disconnected) and a Close event both map to a value; an
/// Error event surfaces as a thrown `SemaError`.
fn event_to_value(ev: Option<WsEvent>) -> Result<Value, SemaError> {
    match ev {
        None => Ok(Value::nil()),
        Some(WsEvent::Text(s)) => Ok(tagged("text", Value::string(&s))),
        Some(WsEvent::Binary(b)) => Ok(tagged("binary", Value::bytevector(b))),
        Some(WsEvent::Close { code, reason }) => {
            let mut info = BTreeMap::new();
            info.insert(Value::keyword("code"), Value::int(code as i64));
            info.insert(Value::keyword("reason"), Value::string(&reason));
            Ok(tagged("close", Value::map(info)))
        }
        Some(WsEvent::Error(e)) => Err(SemaError::Io(e)),
    }
}

/// Check the VM-owned incoming queue without moving its receiver to an executor
/// future. Queue readiness is tested before the deadline so an event already in
/// the channel wins a timeout race.
fn poll_ws_event(
    evt_rx: &Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    deadline: Option<Instant>,
) -> Result<Option<Value>, SemaError> {
    use tokio::sync::mpsc::error::TryRecvError;

    match evt_rx.borrow_mut().try_recv() {
        Ok(event) => event_to_value(Some(event)).map(Some),
        Err(TryRecvError::Disconnected) => Ok(Some(Value::nil())),
        Err(TryRecvError::Empty) if deadline.is_some_and(|at| Instant::now() >= at) => {
            Ok(Some(Value::keyword("timeout")))
        }
        Err(TryRecvError::Empty) => Ok(None),
    }
}

/// Wait until the pump advances or closes its generation channel, or until the
/// optional receive deadline. Every outcome is a wake: the VM continuation owns
/// the policy and rechecks the message queue before deciding what to return.
async fn wait_for_ws_generation(
    mut generation: watch::Receiver<u64>,
    remaining: Option<Duration>,
) -> Result<(), String> {
    match remaining {
        Some(remaining) => {
            tokio::select! {
                _ = generation.changed() => {}
                _ = tokio::time::sleep(remaining) => {}
            }
        }
        None => {
            let _ = generation.changed().await;
        }
    }
    Ok(())
}

fn arm_ws_generation(generation: &watch::Receiver<u64>) -> watch::Receiver<u64> {
    let mut armed = generation.clone();
    armed.borrow_and_update();
    armed
}

/// Arm a lossless generation wait, then perform the final VM-thread queue check.
/// An event published before the generation snapshot is visible in the queue;
/// an event published after it makes `changed()` ready.
fn suspend_ws_receive(
    evt_rx: Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    evt_generation: watch::Receiver<u64>,
    deadline: Option<Instant>,
) -> NativeResult {
    let wait_generation = arm_ws_generation(&evt_generation);

    if let Some(value) = poll_ws_event(&evt_rx, deadline)? {
        return Ok(NativeOutcome::Return(value));
    }

    let remaining = deadline.map(|at| at.saturating_duration_since(Instant::now()));
    let continuation: Box<dyn NativeContinuation> = Box::new(WsRecvContinuation {
        evt_rx,
        evt_generation,
        deadline,
    });
    let kind = CompletionKind::try_from_raw(WS_COMPLETION_KIND)
        .expect("websocket completion kind is nonzero");
    crate::runtime_offload::external_io_async_try_with_continuation(
        "ws/recv",
        kind,
        "ws/recv/generation",
        |()| Ok(Value::nil()),
        continuation,
        move || wait_for_ws_generation(wait_generation, remaining),
    )
}

/// Build a single-key tagged map `{:<key> value}` (`{:text "hi"}`, `{:binary …}`).
fn tagged(key: &str, value: Value) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword(key), value);
    Value::map(m)
}

/// Build the upgrade request for `url`, applying custom headers and subprotocols.
fn build_request(
    url: &str,
    opts: &ConnectOpts,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};

    let mut request = url
        .into_client_request()
        .map_err(|e| format!("ws/connect {url}: {e}"))?;
    let headers = request.headers_mut();
    if !opts.subprotocols.is_empty() {
        let joined = opts.subprotocols.join(", ");
        let val = HeaderValue::from_str(&joined)
            .map_err(|e| format!("ws/connect: invalid subprotocol: {e}"))?;
        headers.insert("Sec-WebSocket-Protocol", val);
    }
    for (k, v) in &opts.headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| format!("ws/connect: invalid header name {k:?}: {e}"))?;
        let val = HeaderValue::from_str(v)
            .map_err(|e| format!("ws/connect: invalid header value: {e}"))?;
        headers.insert(name, val);
    }
    Ok(request)
}

/// Connect, honoring the handshake timeout and retrying with exponential backoff
/// up to `opts.retries` times. A malformed header/URL fails fast (not retried).
async fn connect_with_retry(
    url: &str,
    opts: &ConnectOpts,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let mut attempt: u32 = 0;
    loop {
        let request = build_request(url, opts)?;
        let fut = tokio_tungstenite::connect_async(request);
        let outcome: Result<_, String> = match opts.timeout_ms {
            Some(ms) => match tokio::time::timeout(std::time::Duration::from_millis(ms), fut).await
            {
                Ok(Ok((ws, _resp))) => Ok(ws),
                Ok(Err(e)) => Err(format!("ws/connect {url}: {e}")),
                Err(_) => Err(format!(
                    "ws/connect {url}: handshake timed out after {ms}ms"
                )),
            },
            None => fut
                .await
                .map(|(ws, _resp)| ws)
                .map_err(|e| format!("ws/connect {url}: {e}")),
        };
        match outcome {
            Ok(ws) => return Ok(ws),
            Err(e) => {
                if attempt >= opts.retries {
                    return Err(e);
                }
                // Exponential backoff, capped at 30s.
                let backoff = opts
                    .backoff_ms
                    .saturating_mul(1u64 << attempt.min(16))
                    .min(30_000);
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                attempt += 1;
            }
        }
    }
}

/// Enqueue an event before publishing its generation so a woken continuation
/// always observes the event in the VM-owned queue.
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

/// The pump task: connect, signal handshake result, then bridge the socket to
/// the command/event channels until either side closes.
async fn pump(
    url: String,
    opts: ConnectOpts,
    mut cmd_rx: mpsc::UnboundedReceiver<WsFrame>,
    evt_tx: mpsc::Sender<WsEvent>,
    evt_generation: watch::Sender<u64>,
    ready_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
) {
    let ws = match connect_with_retry(&url, &opts).await {
        Ok(ws) => {
            let _ = ready_tx.send(Ok(()));
            ws
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            evt_generation.send_modify(|value| *value = value.wrapping_add(1));
            return;
        }
    };

    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(WsFrame::Text(s)) => {
                    if sink.send(Message::Text(s.into())).await.is_err() { break; }
                }
                Some(WsFrame::Binary(b)) => {
                    if sink.send(Message::Binary(b.into())).await.is_err() { break; }
                }
                Some(WsFrame::Ping(p)) => {
                    if sink.send(Message::Ping(p.into())).await.is_err() { break; }
                }
                // Explicit close, or the evaluator dropped the connection.
                Some(WsFrame::Close) | None => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    if !publish_event(
                        &evt_tx,
                        &evt_generation,
                        WsEvent::Text(t.to_string()),
                    )
                    .await
                    {
                        break;
                    }
                }
                Some(Ok(Message::Binary(b))) => {
                    if !publish_event(&evt_tx, &evt_generation, WsEvent::Binary(b.to_vec())).await {
                        break;
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = match frame {
                        Some(f) => (u16::from(f.code), f.reason.to_string()),
                        None => (1005, String::new()), // 1005: no status present
                    };
                    let _ = publish_event(
                        &evt_tx,
                        &evt_generation,
                        WsEvent::Close { code, reason },
                    )
                    .await;
                    break;
                }
                // Ping/Pong/raw frames: tungstenite auto-replies to pings.
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    let _ = publish_event(
                        &evt_tx,
                        &evt_generation,
                        WsEvent::Error(format!("websocket: {e}")),
                    )
                    .await;
                    break;
                }
                // Stream ended with no close frame (abnormal).
                None => {
                    let _ = publish_event(
                        &evt_tx,
                        &evt_generation,
                        WsEvent::Close {
                            code: 1006,
                            reason: "connection closed".to_string(),
                        },
                    )
                    .await;
                    break;
                }
            },
        }
    }
    evt_generation.send_modify(|value| *value = value.wrapping_add(1));
}

/// `ws/connect`: spawn the pump, then await the handshake (block at top level,
/// suspend structurally inside an async task). Returns the connection stream.
fn ws_connect(url: &str, opts: ConnectOpts) -> NativeResult {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<WsFrame>();
    let (evt_tx, evt_rx) = mpsc::channel::<WsEvent>(EVENT_CAP);
    let (evt_generation_tx, evt_generation) = watch::channel(0_u64);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

    let abort_pump = sema_io::io_spawn(Box::pin(pump(
        url.to_string(),
        opts,
        cmd_rx,
        evt_tx,
        evt_generation_tx,
        ready_tx,
    )));

    let conn_val = Value::stream(WsConnection {
        cmd_tx,
        evt_rx: Rc::new(RefCell::new(evt_rx)),
        evt_generation,
    });

    // Unified-runtime quantum: await the handshake signal directly. The Send
    // future owns only the oneshot receiver; the traced continuation retains the
    // connection value on the VM thread.
    if sema_core::in_runtime_quantum() {
        let kind = CompletionKind::try_from_raw(WS_COMPLETION_KIND)
            .expect("websocket completion kind is nonzero");
        return crate::runtime_offload::external_io_async_try_with_continuation(
            "ws/connect",
            kind,
            "ws/connect/handshake",
            |handshake: Result<(), String>| match handshake {
                Ok(()) => Ok(Value::nil()),
                Err(message) => Err(SemaError::eval(message)),
            },
            Box::new(WsConnectContinuation {
                conn: conn_val,
                abort_pump: Some(abort_pump),
            }),
            move || async move {
                Ok(match ready_rx.await {
                    Ok(handshake) => handshake,
                    Err(_) => {
                        Err("ws/connect: connection worker dropped before handshake".to_string())
                    }
                })
            },
        );
    }

    // Top level: block the VM thread on the handshake (it is not inside a runtime).
    match ready_rx.blocking_recv() {
        Ok(Ok(())) => Ok(NativeOutcome::Return(conn_val)),
        Ok(Err(msg)) => Err(SemaError::Io(msg)),
        Err(_) => Err(SemaError::eval(
            "ws/connect: connection worker dropped before handshake",
        )),
    }
}

fn ws_send(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ws/send", 2);
    let sb = ws_conn(args, "ws/send", 0)?;
    if sb.is_closed() {
        return Err(SemaError::eval("ws/send: connection is closed"));
    }
    let frame = value_to_frame(&args[1])?;
    let (cmd_tx, _, _) = handles_of(&sb);
    cmd_tx.send(frame).map_err(|_| {
        SemaError::eval("ws/send: connection is closed")
            .with_hint("the websocket has stopped (server closed the connection or it errored)")
    })?;
    Ok(Value::nil())
}

fn ws_recv(args: &[Value]) -> NativeResult {
    check_arity!(args, "ws/recv", 1);
    let sb = ws_conn(args, "ws/recv", 0)?;
    let (_, evt_rx, evt_generation) = handles_of(&sb);

    // Unified-runtime quantum: recheck the VM-owned queue after each lossless
    // generation wake. Cancellation drops only the watch-receiver clone.
    if sema_core::in_runtime_quantum() {
        return suspend_ws_receive(evt_rx, evt_generation, None);
    }

    // Top level: block until an event arrives or the channel disconnects.
    let ev = evt_rx.borrow_mut().blocking_recv();
    event_to_value(ev).map(NativeOutcome::Return)
}

fn ws_recv_timeout(args: &[Value]) -> NativeResult {
    check_arity!(args, "ws/recv-timeout", 2);
    let sb = ws_conn(args, "ws/recv-timeout", 0)?;
    let ms = args[1]
        .as_int()
        .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))
        .map_err(|e| e.with_hint("ws/recv-timeout: argument 2 is the timeout in milliseconds"))?
        .max(0) as u64;
    let (_, evt_rx, evt_generation) = handles_of(&sb);

    // Unified-runtime quantum: race the next generation wake against the exact
    // deadline, rechecking the queue before resolving a timeout.
    if sema_core::in_runtime_quantum() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        return suspend_ws_receive(evt_rx, evt_generation, Some(deadline));
    }

    // Top level: poll on the shared runtime until an event arrives or the deadline
    // passes. We `try_recv` (dropping the RefCell borrow) and `sleep` between polls
    // rather than holding the receiver borrow across an await. `:timeout`
    // distinguishes a timeout from `nil` (the connection closed).
    use tokio::sync::mpsc::error::TryRecvError;
    enum Outcome {
        Event(WsEvent),
        Closed,
        Timeout,
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    let outcome = sema_io::io_block_on(async {
        loop {
            let polled = evt_rx.borrow_mut().try_recv();
            match polled {
                Ok(ev) => return Outcome::Event(ev),
                Err(TryRecvError::Disconnected) => return Outcome::Closed,
                Err(TryRecvError::Empty) => {
                    if std::time::Instant::now() >= deadline {
                        return Outcome::Timeout;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }
        }
    });
    match outcome {
        Outcome::Event(ev) => event_to_value(Some(ev)),
        Outcome::Closed => Ok(Value::nil()),
        Outcome::Timeout => Ok(Value::keyword("timeout")),
    }
    .map(NativeOutcome::Return)
}

fn ws_ping(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ws/ping", 1..=2);
    let sb = ws_conn(args, "ws/ping", 0)?;
    if sb.is_closed() {
        return Err(SemaError::eval("ws/ping: connection is closed"));
    }
    // Optional payload: a string or bytevector. Default is an empty ping.
    let payload = match args.get(1) {
        None => Vec::new(),
        Some(v) => {
            if let Some(s) = v.as_str() {
                s.as_bytes().to_vec()
            } else if let Some(bv) = v.as_bytevector_rc() {
                bv.to_vec()
            } else {
                return Err(SemaError::type_error("string or bytevector", v.type_name())
                    .with_hint("ws/ping payload must be a string or bytevector"));
            }
        }
    };
    let (cmd_tx, _, _) = handles_of(&sb);
    cmd_tx
        .send(WsFrame::Ping(payload))
        .map_err(|_| SemaError::eval("ws/ping: connection is closed"))?;
    Ok(Value::nil())
}

fn ws_close(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ws/close", 1);
    let sb = ws_conn(args, "ws/close", 0)?;
    sb.close()?;
    Ok(Value::nil())
}

fn ws_connected(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ws/connected?", 1);
    let sb = ws_conn(args, "ws/connected?", 0)?;
    let (cmd_tx, _, _) = handles_of(&sb);
    Ok(Value::bool(!sb.is_closed() && !cmd_tx.is_closed()))
}

/// Parse the optional `ws/connect` opts map:
/// `{:headers {…} :subprotocols [...] :timeout ms :retries n :retry-backoff-ms ms}`.
fn parse_connect_opts(v: Option<&Value>) -> Result<ConnectOpts, SemaError> {
    let mut opts = ConnectOpts {
        backoff_ms: 500,
        ..ConnectOpts::default()
    };
    let Some(map) = v.and_then(|v| v.as_map_rc()) else {
        return Ok(opts);
    };

    if let Some(h) = map
        .get(&Value::keyword("headers"))
        .and_then(|h| h.as_map_rc())
    {
        for (k, val) in h.iter() {
            let key = match k.view() {
                sema_core::ValueView::String(s) => s.to_string(),
                sema_core::ValueView::Keyword(s) => sema_core::resolve(s),
                _ => k.to_string(),
            };
            let value = val
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| val.to_string());
            opts.headers.push((key, value));
        }
    }
    if let Some(sp) = map.get(&Value::keyword("subprotocols")) {
        if let Some(list) = sp.as_list_rc().or_else(|| sp.as_vector_rc()) {
            for item in list.iter() {
                if let Some(s) = item.as_str() {
                    opts.subprotocols.push(s.to_string());
                }
            }
        }
    }
    if let Some(ms) = map.get(&Value::keyword("timeout")).and_then(|t| t.as_int()) {
        opts.timeout_ms = Some(ms.max(0) as u64);
    }
    if let Some(n) = map.get(&Value::keyword("retries")).and_then(|t| t.as_int()) {
        opts.retries = n.max(0) as u32;
    }
    if let Some(ms) = map
        .get(&Value::keyword("retry-backoff-ms"))
        .and_then(|t| t.as_int())
    {
        opts.backoff_ms = ms.max(0) as u64;
    }
    Ok(opts)
}

/// Register a non-gated dual-ABI ws native whose body returns `NativeResult` (so
/// its runtime-quantum branch can `NativeOutcome::Suspend`). The plain value
/// callback unwraps the `Return` produced outside the runtime.
fn register_runtime_fn(env: &sema_core::Env, name: &'static str, f: fn(&[Value]) -> NativeResult) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            move |args| match f(args)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(SemaError::eval(format!(
                    "{name}: native suspended outside the cooperative runtime"
                ))),
            },
            move |_native_ctx, args| f(args),
        )),
    );
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // Establishing a connection touches the network → gate on NETWORK. The
    // per-message ops below operate on an already-open connection (which could
    // only be obtained through this gate), so they need no separate gate —
    // matching the server-side ws closures.
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::NETWORK, "ws/connect", &[], |args| {
        check_arity!(args, "ws/connect", 1..=2);
        let url = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if !(url.starts_with("ws://") || url.starts_with("wss://")) {
            return Err(
                SemaError::eval(format!("ws/connect: not a websocket URL: {url}"))
                    .with_hint("the URL must start with ws:// or wss://"),
            );
        }
        let opts = parse_connect_opts(args.get(1))?;
        ws_connect(url, opts)
    });

    register_fn(env, "ws/send", ws_send);
    register_runtime_fn(env, "ws/recv", ws_recv);
    register_runtime_fn(env, "ws/recv-timeout", ws_recv_timeout);
    register_fn(env, "ws/ping", ws_ping);
    register_fn(env, "ws/close", ws_close);
    register_fn(env, "ws/connected?", ws_connected);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::runtime::{
        CancellationView, NativeCallContext, NativeContinuation, ResumeInput, TaskContext, WaitKind,
    };
    use std::time::Instant;
    use tokio::sync::watch;

    #[test]
    fn ws_generation_wakes_after_published_event() {
        let (evt_tx, mut evt_rx) = mpsc::channel(1);
        let (generation_tx, generation_rx) = watch::channel(0_u64);
        let mut wait_generation = generation_rx.clone();
        wait_generation.borrow_and_update();

        sema_io::io_block_on(async {
            assert!(
                publish_event(&evt_tx, &generation_tx, WsEvent::Text("ready".to_string()),).await
            );
            tokio::time::timeout(
                Duration::from_millis(100),
                wait_for_ws_generation(wait_generation, None),
            )
            .await
            .expect("published event must advance the generation")
            .expect("generation wait must succeed");
        });

        assert!(matches!(
            evt_rx.try_recv(),
            Ok(WsEvent::Text(text)) if text == "ready"
        ));
    }

    #[test]
    fn ws_generation_retains_change_before_future_is_polled() {
        let (generation_tx, generation_rx) = watch::channel(0_u64);
        let wait_future = wait_for_ws_generation(generation_rx, None);
        generation_tx.send_modify(|generation| *generation += 1);

        sema_io::io_block_on(async {
            tokio::time::timeout(Duration::from_millis(100), wait_future)
                .await
                .expect("an unpolled wait must retain the generation change")
                .expect("generation wait must succeed");
        });
    }

    #[test]
    fn ws_generation_rearm_waits_for_subsequent_change() {
        let (generation_tx, generation_rx) = watch::channel(0_u64);
        generation_tx.send_modify(|generation| *generation += 1);
        let armed_generation = arm_ws_generation(&generation_rx);

        sema_io::io_block_on(async {
            let mut wait = Box::pin(wait_for_ws_generation(armed_generation, None));
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut wait)
                    .await
                    .is_err(),
                "a coalesced generation must be marked seen before rearming"
            );
            generation_tx.send_modify(|generation| *generation += 1);
            tokio::time::timeout(Duration::from_millis(100), wait)
                .await
                .expect("a subsequent generation must wake the rearmed wait")
                .expect("generation wait must succeed");
        });
    }

    #[test]
    fn dropping_ws_wait_preserves_receiver() {
        let (evt_tx, mut evt_rx) = mpsc::channel(1);
        let (_generation_tx, generation_rx) = watch::channel(0_u64);
        let wait_future = wait_for_ws_generation(generation_rx, None);

        drop(wait_future);
        evt_tx
            .try_send(WsEvent::Text("still-open".to_string()))
            .unwrap();
        assert!(matches!(
            evt_rx.try_recv(),
            Ok(WsEvent::Text(text)) if text == "still-open"
        ));
    }

    #[test]
    fn ws_generation_closed_sender_wakes_pending_wait() {
        let (generation_tx, generation_rx) = watch::channel(0_u64);

        sema_io::io_block_on(async {
            let waiter = tokio::spawn(wait_for_ws_generation(generation_rx, None));
            tokio::task::yield_now().await;
            drop(generation_tx);
            tokio::time::timeout(Duration::from_millis(100), waiter)
                .await
                .expect("closing the generation sender must wake a pending wait")
                .expect("generation waiter task must not panic")
                .expect("a closed sender is a readiness wake, not a wait error");
        });
    }

    #[test]
    fn ws_connect_continuation_traces_and_returns_connection() {
        let connection = Value::int(41);
        let continuation = WsConnectContinuation {
            conn: connection.clone(),
            abort_pump: Some(Box::new(|| {
                panic!("successful handshake must disarm its pump abort hook")
            })),
        };
        let mut edges = 0;
        assert!(continuation.trace(&mut |_| edges += 1));
        assert_eq!(edges, 1);

        let eval_context = sema_core::EvalContext::new();
        let mut task_context = TaskContext::empty();
        let mut native_context = NativeCallContext {
            eval_context: &eval_context,
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };
        let outcome = Box::new(continuation)
            .resume(&mut native_context, ResumeInput::Returned(Value::nil()))
            .expect("successful handshake continuation must resume");
        assert!(matches!(outcome, NativeOutcome::Return(value) if value == connection));
    }

    #[test]
    fn ws_connect_continuation_drop_aborts_spawned_task() {
        use std::sync::mpsc;

        struct SignalOnDrop(Option<mpsc::SyncSender<()>>);

        impl Drop for SignalOnDrop {
            fn drop(&mut self) {
                if let Some(signal) = self.0.take() {
                    let _ = signal.send(());
                }
            }
        }

        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
        let abort_pump = sema_io::io_spawn(async move {
            let _drop_signal = SignalOnDrop(Some(dropped_tx));
            started_tx.send(()).expect("report spawned task start");
            futures::future::pending::<()>().await;
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("spawned task must start");

        drop(WsConnectContinuation {
            conn: Value::nil(),
            abort_pump: Some(abort_pump),
        });

        dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("dropping the continuation must abort its spawned task");
    }

    #[test]
    fn ws_recv_continuation_traces_no_values() {
        let (_evt_tx, evt_rx) = mpsc::channel(1);
        let (_generation_tx, evt_generation) = watch::channel(0_u64);
        let continuation = WsRecvContinuation {
            evt_rx: Rc::new(RefCell::new(evt_rx)),
            evt_generation,
            deadline: None,
        };
        let mut edges = 0;
        assert!(continuation.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0);
    }

    #[test]
    fn ws_timeout_queued_event_wins_expired_deadline() {
        let (evt_tx, evt_rx) = mpsc::channel(1);
        evt_tx
            .try_send(WsEvent::Text("on-the-line".to_string()))
            .unwrap();
        let evt_rx = Rc::new(RefCell::new(evt_rx));
        let deadline = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();

        let value = poll_ws_event(&evt_rx, Some(deadline))
            .expect("queued event must decode")
            .expect("queued event must be ready");
        let expected = event_to_value(Some(WsEvent::Text("on-the-line".to_string()))).unwrap();
        assert_eq!(value, expected);
    }

    #[test]
    fn ws_timeout_empty_queue_returns_timeout() {
        let (_evt_tx, evt_rx) = mpsc::channel(1);
        let evt_rx = Rc::new(RefCell::new(evt_rx));
        let deadline = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();

        assert_eq!(
            poll_ws_event(&evt_rx, Some(deadline)).expect("empty timeout poll must succeed"),
            Some(Value::keyword("timeout"))
        );
    }

    #[test]
    fn ws_recv_continuation_rearms_after_coalesced_empty_wake() {
        let (_evt_tx, evt_rx) = mpsc::channel(1);
        let (generation_tx, evt_generation) = watch::channel(0_u64);
        generation_tx.send_modify(|generation| *generation += 1);
        let continuation = WsRecvContinuation {
            evt_rx: Rc::new(RefCell::new(evt_rx)),
            evt_generation,
            deadline: None,
        };
        let eval_context = sema_core::EvalContext::new();
        let mut task_context = TaskContext::empty();
        let mut native_context = NativeCallContext {
            eval_context: &eval_context,
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };

        let outcome = Box::new(continuation)
            .resume(&mut native_context, ResumeInput::Returned(Value::nil()))
            .expect("coalesced wake must recheck and rearm");
        let NativeOutcome::Suspend(suspend) = outcome else {
            panic!("an empty queue after a coalesced wake must suspend again");
        };
        assert!(matches!(suspend.wait, WaitKind::External(_)));
    }
}
