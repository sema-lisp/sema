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
//! yield `AwaitIo` so sibling tasks run while a recv/handshake is in flight.

#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use sema_core::{check_arity, Caps, IoHandle, IoPoll, SemaError, SemaStream, StreamBox, Value};

use crate::register_fn;

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
) {
    let inner = sb.borrow_inner();
    let conn = inner.as_any().downcast_ref::<WsConnection>().unwrap();
    (conn.cmd_tx.clone(), conn.evt_rx.clone())
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

/// The pump task: connect, signal handshake result, then bridge the socket to
/// the command/event channels until either side closes.
async fn pump(
    url: String,
    opts: ConnectOpts,
    mut cmd_rx: mpsc::UnboundedReceiver<WsFrame>,
    evt_tx: mpsc::Sender<WsEvent>,
    ready_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
) {
    let ws = match connect_with_retry(&url, &opts).await {
        Ok(ws) => {
            let _ = ready_tx.send(Ok(()));
            ws
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
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
                    if evt_tx.send(WsEvent::Text(t.to_string())).await.is_err() { break; }
                    sema_core::notify_io_complete();
                }
                Some(Ok(Message::Binary(b))) => {
                    if evt_tx.send(WsEvent::Binary(b.to_vec())).await.is_err() { break; }
                    sema_core::notify_io_complete();
                }
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = match frame {
                        Some(f) => (u16::from(f.code), f.reason.to_string()),
                        None => (1005, String::new()), // 1005: no status present
                    };
                    let _ = evt_tx.send(WsEvent::Close { code, reason }).await;
                    sema_core::notify_io_complete();
                    break;
                }
                // Ping/Pong/raw frames: tungstenite auto-replies to pings.
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    let _ = evt_tx.send(WsEvent::Error(format!("websocket: {e}"))).await;
                    sema_core::notify_io_complete();
                    break;
                }
                // Stream ended with no close frame (abnormal).
                None => {
                    let _ = evt_tx
                        .send(WsEvent::Close { code: 1006, reason: "connection closed".to_string() })
                        .await;
                    sema_core::notify_io_complete();
                    break;
                }
            },
        }
    }
}

/// `ws/connect`: spawn the pump, then await the handshake (block at top level,
/// yield `AwaitIo` inside an async task). Returns the connection stream value.
fn ws_connect(url: &str, opts: ConnectOpts) -> Result<Value, SemaError> {
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value), kept
    // for symmetry with the shipped async yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<WsFrame>();
    let (evt_tx, evt_rx) = mpsc::channel::<WsEvent>(EVENT_CAP);
    let (ready_tx, mut ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

    let abort_pump = sema_io::io_spawn(Box::pin(pump(
        url.to_string(),
        opts,
        cmd_rx,
        evt_tx,
        ready_tx,
    )));

    let conn_val = Value::stream(WsConnection {
        cmd_tx,
        evt_rx: Rc::new(RefCell::new(evt_rx)),
    });

    if sema_core::in_async_context() {
        // Park until the handshake completes; the pump's per-event notify wakes us.
        let conn_for_poll = conn_val.clone();
        let handle = Rc::new(IoHandle::with_abort(
            move || match ready_rx.try_recv() {
                Err(TryRecvError::Empty) => IoPoll::Pending,
                Ok(Ok(())) => IoPoll::Ready(Ok(conn_for_poll.clone())),
                Ok(Err(msg)) => IoPoll::Ready(Err(msg)),
                Err(TryRecvError::Closed) => {
                    IoPoll::Ready(Err("ws/connect: connection worker dropped".to_string()))
                }
            },
            abort_pump,
        ));
        sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
        return Ok(Value::nil());
    }

    // Top level: block the VM thread on the handshake (it is not inside a runtime).
    match ready_rx.blocking_recv() {
        Ok(Ok(())) => Ok(conn_val),
        Ok(Err(msg)) => Err(SemaError::Io(msg)),
        Err(_) => Err(SemaError::eval(
            "ws/connect: connection worker dropped before handshake",
        )),
    }
}

/// The async-context recv path: yield an `AwaitIo` whose poller drains one event.
fn ws_recv_async(evt_rx: Rc<RefCell<mpsc::Receiver<WsEvent>>>) -> Result<Value, SemaError> {
    use tokio::sync::mpsc::error::TryRecvError;

    let handle = Rc::new(IoHandle::new(move || {
        match evt_rx.borrow_mut().try_recv() {
            Ok(ev) => match event_to_value(Some(ev)) {
                Ok(v) => IoPoll::Ready(Ok(v)),
                Err(e) => IoPoll::Ready(Err(e.to_string())),
            },
            Err(TryRecvError::Empty) => IoPoll::Pending,
            Err(TryRecvError::Disconnected) => IoPoll::Ready(Ok(Value::nil())),
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

fn ws_send(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ws/send", 2);
    let sb = ws_conn(args, "ws/send", 0)?;
    if sb.is_closed() {
        return Err(SemaError::eval("ws/send: connection is closed"));
    }
    let frame = value_to_frame(&args[1])?;
    let (cmd_tx, _) = handles_of(&sb);
    cmd_tx.send(frame).map_err(|_| {
        SemaError::eval("ws/send: connection is closed")
            .with_hint("the websocket has stopped (server closed the connection or it errored)")
    })?;
    Ok(Value::nil())
}

fn ws_recv(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ws/recv", 1);
    let sb = ws_conn(args, "ws/recv", 0)?;
    let (_, evt_rx) = handles_of(&sb);

    if sema_core::in_async_context() {
        if let Some(v) = sema_core::take_resume_value() {
            return Ok(v);
        }
        return ws_recv_async(evt_rx);
    }

    // Top level: block until an event arrives or the channel disconnects.
    let ev = evt_rx.borrow_mut().blocking_recv();
    event_to_value(ev)
}

/// The async-context recv-with-deadline path. Polls for an event, returning the
/// `:timeout` keyword once `deadline` passes with nothing received.
fn ws_recv_timeout_async(
    evt_rx: Rc<RefCell<mpsc::Receiver<WsEvent>>>,
    deadline: std::time::Instant,
) -> Result<Value, SemaError> {
    use tokio::sync::mpsc::error::TryRecvError;

    let handle = Rc::new(IoHandle::new(move || {
        match evt_rx.borrow_mut().try_recv() {
            Ok(ev) => match event_to_value(Some(ev)) {
                Ok(v) => IoPoll::Ready(Ok(v)),
                Err(e) => IoPoll::Ready(Err(e.to_string())),
            },
            Err(TryRecvError::Disconnected) => IoPoll::Ready(Ok(Value::nil())),
            Err(TryRecvError::Empty) => {
                if std::time::Instant::now() >= deadline {
                    IoPoll::Ready(Ok(Value::keyword("timeout")))
                } else {
                    IoPoll::Pending
                }
            }
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

fn ws_recv_timeout(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ws/recv-timeout", 2);
    let sb = ws_conn(args, "ws/recv-timeout", 0)?;
    let ms = args[1]
        .as_int()
        .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))
        .map_err(|e| e.with_hint("ws/recv-timeout: argument 2 is the timeout in milliseconds"))?
        .max(0) as u64;
    let (_, evt_rx) = handles_of(&sb);

    if sema_core::in_async_context() {
        if let Some(v) = sema_core::take_resume_value() {
            return Ok(v);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        return ws_recv_timeout_async(evt_rx, deadline);
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
    let (cmd_tx, _) = handles_of(&sb);
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
    let (cmd_tx, _) = handles_of(&sb);
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

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // Establishing a connection touches the network → gate on NETWORK. The
    // per-message ops below operate on an already-open connection (which could
    // only be obtained through this gate), so they need no separate gate —
    // matching the server-side ws closures.
    crate::register_fn_gated(env, sandbox, Caps::NETWORK, "ws/connect", |args| {
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
    register_fn(env, "ws/recv", ws_recv);
    register_fn(env, "ws/recv-timeout", ws_recv_timeout);
    register_fn(env, "ws/ping", ws_ping);
    register_fn(env, "ws/close", ws_close);
    register_fn(env, "ws/connected?", ws_connected);
}
