//! One bounded FIFO journal writer thread per run (A3 — bounded workflow I/O).
//!
//! The VM thread NEVER touches the filesystem for a journal write: it renders each
//! event / memo / sidecar to a plain `String` (or JSON) ON THE VM THREAD and
//! `try_send`s it to this writer, which owns the `events.jsonl` handle and performs
//! every `fs` write on its own named OS thread. This keeps the cooperative VM quantum
//! free of blocking disk I/O and of full-value materialization while still preserving
//! the frozen append-only journal contract.
//!
//! Best-effort, never-blocking, never-crashing (the same trust model as the retired
//! synchronous path, just off the VM thread):
//!
//! * A full queue DROPS the message and bumps a VM-side counter; the next successful
//!   enqueue first emits one `journal.overflow` event carrying the dropped count
//!   (an append-only vocabulary addition — golden runs are far below the bound, so
//!   they never emit it and stay byte-identical).
//! * A writer-side odometer caps total `events.jsonl` bytes; past the cap the writer
//!   records ONE `journal.overflow` marker and drops further events (the on-disk
//!   prefix stays valid JSONL).
//! * A terminal `Flush` barrier lets `workflow/run` park until every message ahead of
//!   it is on disk, so a NORMAL run return means the journal is complete.
//!
//! The writer is RESOURCE-OWNED by the run: [`JournalWriter`]'s `Drop` `try_send`s
//! `Stop` and DETACHES — it never `join`s, so a cancellation / teardown path can never
//! block on the writer. The thread also exits on channel disconnect, draining whatever
//! is already queued first.

use std::cell::Cell;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Condvar, Mutex, OnceLock};

/// Default bound on the in-flight journal message queue. A run journals a handful of
/// events, FAR below this, so a full queue only happens under a pathological burst —
/// where dropping (never blocking the VM thread) is the documented best-effort
/// contract. Overridable per-run via the `SEMA_WORKFLOW_JOURNAL_QUEUE` seam (tests set
/// it tiny to exercise overflow deterministically).
const DEFAULT_QUEUE_BOUND: usize = 4096;

/// Default writer-side odometer cap on total `events.jsonl` bytes. Past this the writer
/// records ONE `journal.overflow` marker and drops further events. Overridable via the
/// `SEMA_WORKFLOW_JOURNAL_MAX_BYTES` seam.
const DEFAULT_TOTAL_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// A message to the journal writer thread. Everything is already rendered to a `Send`
/// `String` on the VM thread — the writer never touches a `Value`/`Env` (CORE-2 I2 +
/// the single-thread VM invariant).
enum WriterMsg {
    /// A rendered `events.jsonl` line (WITHOUT a trailing newline — the writer appends
    /// it, so the odometer and marker share one code path).
    Event(String),
    /// A per-leaf memo sidecar: `memo/<key>.json` ← `json`.
    Memo { key: String, json: String },
    /// A whole-file sidecar (`args.json` / `metadata.json` / `result.json`) ← `json`.
    Sidecar { name: String, json: String },
    /// Flush the buffered writer to disk, then acknowledge on the paired channel — the
    /// terminal durability barrier a normal `workflow/run` return parks on.
    Flush(Sender<()>),
    /// Stop the writer (Journal drop). The writer also stops on channel disconnect.
    Stop,
}

/// The VM-thread handle to a run's journal writer thread. Cheap: a bounded sender plus a
/// queue-full drop counter. Owned by the [`crate::Journal`]; performs NO filesystem I/O
/// itself (that all happens on the writer thread).
pub struct JournalWriter {
    tx: SyncSender<WriterMsg>,
    /// Count of event messages dropped because the queue was full, surfaced as one
    /// `journal.overflow` event the next time an enqueue succeeds.
    dropped: Cell<u64>,
}

impl JournalWriter {
    /// Spawn the writer thread owning `file` (`events.jsonl`) under run dir `dir`. Reads
    /// the queue-bound / total-bytes seams ON THE VM THREAD and hands the resolved values
    /// to the thread (no env reads off-thread).
    pub fn spawn(dir: PathBuf, file: File) -> Self {
        let bound = env_usize("SEMA_WORKFLOW_JOURNAL_QUEUE", DEFAULT_QUEUE_BOUND).max(1);
        let max_bytes = env_u64("SEMA_WORKFLOW_JOURNAL_MAX_BYTES", DEFAULT_TOTAL_MAX_BYTES);
        let (tx, rx) = sync_channel::<WriterMsg>(bound);
        // Best-effort spawn: a spawn failure (astronomically unlikely) leaves `tx` with no
        // receiver, so every enqueue is a no-op drop — journaling degrades, the run still
        // runs. Never `join`ed (see the module doc); the handle is dropped/detached.
        let _ = std::thread::Builder::new()
            .name("sema-wf-journal".to_string())
            .spawn(move || writer_loop(dir, file, max_bytes, rx));
        Self {
            tx,
            dropped: Cell::new(0),
        }
    }

    /// Enqueue a rendered event line (no trailing newline). NEVER blocks the VM thread: a
    /// full queue drops the line and bumps the overflow counter; a subsequent successful
    /// enqueue first emits one `journal.overflow` marker carrying the dropped count.
    pub fn enqueue_event(&self, line: String) {
        let dropped = self.dropped.get();
        if dropped > 0 {
            let marker =
                format!(r#"{{"event":"journal.overflow","reason":"queue-full","dropped":{dropped}}}"#);
            if self.tx.try_send(WriterMsg::Event(marker)).is_ok() {
                self.dropped.set(0);
            }
        }
        if let Err(TrySendError::Full(_)) = self.tx.try_send(WriterMsg::Event(line)) {
            self.dropped.set(self.dropped.get() + 1);
        }
    }

    /// Enqueue a per-leaf memo write (best-effort; a full queue drops it and the leaf
    /// re-runs on resume — identical semantics to the round-trip guard).
    pub fn enqueue_memo(&self, key: String, json: String) {
        let _ = self.tx.try_send(WriterMsg::Memo { key, json });
    }

    /// Enqueue a whole-file sidecar write (`args.json` / `metadata.json` / `result.json`).
    pub fn enqueue_sidecar(&self, name: String, json: String) {
        let _ = self.tx.try_send(WriterMsg::Sidecar { name, json });
    }

    /// Enqueue a terminal flush barrier; the returned receiver resolves once the writer
    /// has flushed every message ahead of it to disk. Best-effort like every enqueue: a
    /// full queue drops the barrier (the caller's bounded wait / park then settles without
    /// the ack — the run already computed its result). Golden runs are far below the
    /// bound, so the barrier always lands and a normal return means the journal is durable.
    pub fn request_flush(&self) -> Receiver<()> {
        let (ack_tx, ack_rx) = channel();
        let _ = self.tx.try_send(WriterMsg::Flush(ack_tx));
        ack_rx
    }
}

impl Drop for JournalWriter {
    fn drop(&mut self) {
        // Ask the writer to stop; NEVER join. The thread also exits on channel disconnect,
        // draining whatever is already queued first, so buffered events still land.
        let _ = self.tx.try_send(WriterMsg::Stop);
    }
}

/// The writer thread body: own the `BufWriter<File>`, drain the FIFO, flush per event
/// (a crash mid-run still leaves a readable prefix), and enforce the byte odometer. All
/// write errors are swallowed — journaling must never abort the run.
fn writer_loop(dir: PathBuf, file: File, max_bytes: u64, rx: Receiver<WriterMsg>) {
    let mut out = BufWriter::new(file);
    let mut written: u64 = 0;
    let mut size_overflowed = false;
    while let Ok(msg) = rx.recv() {
        wait_if_stalled();
        match msg {
            WriterMsg::Event(mut line) => {
                line.push('\n');
                let cost = line.len() as u64;
                if size_overflowed {
                    continue;
                }
                if written + cost > max_bytes {
                    // Total-bytes cap tripped: record ONE marker, then drop further events.
                    size_overflowed = true;
                    let marker = format!(
                        "{{\"event\":\"journal.overflow\",\"reason\":\"journal-size-cap\",\"bytes\":{written}}}\n"
                    );
                    let _ = out.write_all(marker.as_bytes());
                    let _ = out.flush();
                    continue;
                }
                let _ = out.write_all(line.as_bytes());
                let _ = out.flush();
                written += cost;
            }
            WriterMsg::Memo { key, json } => {
                let memo_dir = dir.join("memo");
                if fs::create_dir_all(&memo_dir).is_ok() {
                    let _ = fs::write(memo_dir.join(format!("{key}.json")), json);
                }
            }
            WriterMsg::Sidecar { name, json } => {
                let _ = fs::write(dir.join(name), json);
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

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ── test-only writer stall gate ──────────────────────────────────────────────
//
// A process-global barrier that, once armed, makes every writer thread block (on a
// `Condvar`, NEVER `thread::sleep`) before draining each message. Tests use it to hold a
// writer stalled while a parked terminal flush-ack is cancelled (proving the task settles
// without joining the writer) and to force queue overflow deterministically. In
// production `STALL_ARMED` is never set, so `wait_if_stalled` is a single relaxed atomic
// load per message.

static STALL_ARMED: AtomicBool = AtomicBool::new(false);
static STALL_STATE: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

fn stall_state() -> &'static (Mutex<bool>, Condvar) {
    STALL_STATE.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

fn wait_if_stalled() {
    if !STALL_ARMED.load(Ordering::Relaxed) {
        return;
    }
    let (lock, cvar) = stall_state();
    let mut stalled = lock.lock().unwrap_or_else(|e| e.into_inner());
    while STALL_ARMED.load(Ordering::Relaxed) && *stalled {
        stalled = cvar.wait(stalled).unwrap_or_else(|e| e.into_inner());
    }
}

/// Test-only: arm the writer stall gate and set whether writers block before draining
/// each message. Never called in production.
#[doc(hidden)]
pub fn __test_set_stall(stalled: bool) {
    let (lock, cvar) = stall_state();
    *lock.lock().unwrap_or_else(|e| e.into_inner()) = stalled;
    STALL_ARMED.store(true, Ordering::SeqCst);
    cvar.notify_all();
}

/// Test-only: disarm the stall gate so every writer drains freely again.
#[doc(hidden)]
pub fn __test_disarm_stall() {
    let (lock, cvar) = stall_state();
    *lock.lock().unwrap_or_else(|e| e.into_inner()) = false;
    STALL_ARMED.store(false, Ordering::SeqCst);
    cvar.notify_all();
}
