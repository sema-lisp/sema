//! A collector-independent JSONL span exporter for `SEMA_OTEL_FILE`.
//!
//! Each finished span is written as one JSON object per line (a Sema-defined stable
//! schema), so offline capture works with no OTLP collector. Preferred over
//! `opentelemetry-stdout`, whose format is explicitly unspecified.
//!
//! **The VM thread never touches the filesystem for a span export.** `export()` renders
//! each finished span to a plain `String` ON THE VM THREAD (bounded — attributes are
//! already per-field truncated upstream) and `try_send`s it to a dedicated writer thread
//! that owns the `events.jsonl`-style handle and performs every `write_all`+`flush` on its
//! own OS thread. This mirrors the workflow journal writer (A3): the cooperative VM quantum
//! stays free of blocking disk I/O while span mutation itself remains synchronous.
//!
//! Best-effort, never-blocking, never-crashing (the same trust model as the retired
//! synchronous path, just off the VM thread):
//!
//! * A full queue DROPS the rendered line (never blocks the emitting thread) — the file
//!   sink is a debugging aid whose contract already tolerates a dropped span under a
//!   pathological burst.
//! * A terminal `Flush` barrier lets provider shutdown park (bounded) until every line
//!   ahead of it is on disk, so once `SdkTracerProvider::shutdown` returns the file is
//!   complete. `SimpleSpanProcessor::shutdown_with_timeout` forwards to the exporter, so
//!   this is the durability point.
//!
//! The writer is owned by the exporter: [`JsonlFileExporter`]'s `Drop` `try_send`s `Stop`
//! and DETACHES — it never `join`s, so no drop/shutdown path can block on the writer from a
//! quantum. The thread also exits on channel disconnect, draining whatever is already
//! queued first.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::{SpanKind, Status};
use opentelemetry::{Array, KeyValue, Value};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};

/// Default bound on the in-flight span-line queue. A traced run emits a handful of spans,
/// FAR below this, so a full queue only happens under a pathological burst — where dropping
/// (never blocking the emitting thread) is the documented best-effort contract. Overridable
/// via the `SEMA_OTEL_FILE_QUEUE` seam.
const DEFAULT_QUEUE_BOUND: usize = 4096;

/// Bounded wait for the terminal flush acknowledgement on `force_flush`. Shutdown uses the
/// timeout the SDK passes (`shutdown_with_timeout`).
const FLUSH_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// A message to the file writer thread. Everything is already rendered to a `Send` `String`
/// on the emitting thread — the writer never touches an OTel `SpanData` (kept off the VM
/// thread) let alone a Sema `Value`/`Env`.
enum WriterMsg {
    /// A rendered `events.jsonl` line (already terminated with `\n`).
    Line(String),
    /// Flush the buffered writer to disk, then acknowledge on the paired channel — the
    /// terminal durability barrier provider shutdown parks on.
    Flush(Sender<()>),
    /// Stop the writer (exporter drop). The writer also stops on channel disconnect.
    Stop,
}

/// Test-only: the id of the most recently spawned file-writer thread, recorded when the
/// writer thread starts. Lets the regression assert that span export runs OFF the emitting
/// (VM) thread. Never read in production. `Mutex` (not `OnceLock`) so a re-init in one
/// process records the latest writer.
static WRITER_THREAD_ID: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

/// Test-only accessor for [`WRITER_THREAD_ID`]. See its docs.
#[doc(hidden)]
pub fn last_writer_thread_id() -> Option<std::thread::ThreadId> {
    WRITER_THREAD_ID.lock().ok().and_then(|g| *g)
}

#[derive(Debug)]
pub struct JsonlFileExporter {
    tx: SyncSender<WriterMsg>,
}

impl JsonlFileExporter {
    /// Open `path` for append (creating it if absent) and spawn the writer thread that owns
    /// it. Errors propagate to the caller so init can fall back to a no-op rather than
    /// panic. The queue-bound seam is read ON THE calling thread (no env reads off-thread).
    pub fn new(path: &str) -> std::io::Result<Self> {
        let file = OpenOptions::new().append(true).create(true).open(path)?;
        let bound = env_usize("SEMA_OTEL_FILE_QUEUE", DEFAULT_QUEUE_BOUND).max(1);
        let (tx, rx) = sync_channel::<WriterMsg>(bound);
        // Best-effort spawn: a spawn failure (astronomically unlikely) leaves `tx` with no
        // receiver, so every enqueue is a no-op drop — capture degrades, the run still runs.
        // Never `join`ed (see the module doc); the handle is dropped/detached.
        let _ = std::thread::Builder::new()
            .name("sema-otel-file".to_string())
            .spawn(move || writer_loop(file, rx));
        Ok(Self { tx })
    }

    /// Render each span to a JSONL line ON THE calling (VM) thread and enqueue it. NEVER
    /// blocks: a full queue drops the line (best-effort file-sink contract).
    fn enqueue_batch(&self, batch: Vec<SpanData>) {
        for span in &batch {
            let mut line = serde_json::to_string(&span_to_json(span))
                .unwrap_or_else(|e| format!("{{\"error\":\"otel serialize: {e}\"}}"));
            line.push('\n');
            let _ = self.tx.try_send(WriterMsg::Line(line));
        }
    }

    /// Enqueue a terminal flush barrier; the returned receiver resolves once the writer has
    /// flushed every message ahead of it to disk. Best-effort: a full queue drops the
    /// barrier (the caller's bounded wait then settles without the ack).
    fn request_flush(&self) -> Receiver<()> {
        let (ack_tx, ack_rx) = channel();
        let _ = self.tx.try_send(WriterMsg::Flush(ack_tx));
        ack_rx
    }

    /// Send a flush barrier and wait, bounded, for its acknowledgement. Runs on provider
    /// shutdown / force-flush (off the VM thread), never inside a cooperative quantum.
    fn bounded_flush(&self, timeout: Duration) {
        let _ = self.request_flush().recv_timeout(timeout);
    }
}

impl Drop for JsonlFileExporter {
    fn drop(&mut self) {
        // Ask the writer to stop; NEVER join. The thread also exits on channel disconnect,
        // draining whatever is already queued first, so buffered lines still land.
        let _ = self.tx.try_send(WriterMsg::Stop);
    }
}

impl SpanExporter for JsonlFileExporter {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        // Render-and-enqueue on the calling thread; the write happens off-thread.
        self.enqueue_batch(batch);
        std::future::ready(Ok(()))
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.bounded_flush(FLUSH_ACK_TIMEOUT);
        Ok(())
    }

    /// The method `SimpleSpanProcessor::shutdown_with_timeout` actually forwards to (the
    /// trait's default `shutdown()` delegates here). A bounded flush makes the file complete
    /// on disk by the time provider shutdown returns.
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.bounded_flush(timeout);
        Ok(())
    }
}

/// The writer thread body: own the `BufWriter<File>`, drain the FIFO, and flush per line (a
/// crash mid-run still leaves a readable JSONL prefix). All write errors are swallowed —
/// span capture must never abort the run.
fn writer_loop(file: File, rx: Receiver<WriterMsg>) {
    if let Ok(mut g) = WRITER_THREAD_ID.lock() {
        *g = Some(std::thread::current().id());
    }
    let mut out = BufWriter::new(file);
    while let Ok(msg) = rx.recv() {
        match msg {
            WriterMsg::Line(line) => {
                let _ = out.write_all(line.as_bytes());
                let _ = out.flush();
            }
            WriterMsg::Flush(ack) => {
                let _ = out.flush();
                let _ = ack.send(());
            }
            WriterMsg::Stop => break,
        }
    }
    let _ = out.flush();
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn nanos(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn kind_str(k: &SpanKind) -> &'static str {
    match k {
        SpanKind::Client => "client",
        SpanKind::Server => "server",
        SpanKind::Producer => "producer",
        SpanKind::Consumer => "consumer",
        SpanKind::Internal => "internal",
    }
}

fn value_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Bool(b) => J::Bool(*b),
        Value::I64(i) => J::from(*i),
        Value::F64(f) => J::from(*f),
        Value::String(s) => J::String(s.to_string()),
        Value::Array(a) => match a {
            Array::Bool(xs) => J::Array(xs.iter().map(|x| J::Bool(*x)).collect()),
            Array::I64(xs) => J::Array(xs.iter().map(|x| J::from(*x)).collect()),
            Array::F64(xs) => J::Array(xs.iter().map(|x| J::from(*x)).collect()),
            Array::String(xs) => J::Array(xs.iter().map(|x| J::String(x.to_string())).collect()),
            _ => J::Null,
        },
        _ => J::Null,
    }
}

fn attrs_to_json(attrs: &[KeyValue]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for kv in attrs {
        map.insert(kv.key.to_string(), value_to_json(&kv.value));
    }
    serde_json::Value::Object(map)
}

/// Serialize one span to the Sema JSONL schema. Public for the in-crate tests.
pub fn span_to_json(span: &SpanData) -> serde_json::Value {
    let status = match &span.status {
        Status::Unset => "unset".to_string(),
        Status::Ok => "ok".to_string(),
        Status::Error { description } => format!("error: {description}"),
    };
    serde_json::json!({
        "name": span.name.to_string(),
        "trace_id": format!("{:032x}", span.span_context.trace_id()),
        "span_id": format!("{:016x}", span.span_context.span_id()),
        "parent_span_id": format!("{:016x}", span.parent_span_id),
        "kind": kind_str(&span.span_kind),
        "start_unix_nano": nanos(span.start_time).to_string(),
        "end_unix_nano": nanos(span.end_time).to_string(),
        "status": status,
        "attributes": attrs_to_json(&span.attributes),
    })
}
