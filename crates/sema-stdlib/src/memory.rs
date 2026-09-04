//! Agent memory threads (`memory/*`) — the transcript half.
//!
//! A memory thread is a persistable, append-only conversation log keyed on
//! `(namespace, id)`. Threads live in a thread-local registry; the durable form
//! is one JSON object per line (`{"role":..,"content":..}`) in
//! `<base>/<namespace>/<id>.jsonl`, where `<base>` is `./.sema/memory` (the
//! same project-local `.sema` convention as `workflow/run`'s
//! `./.sema/runs/<run-id>/`), overridable for tests via
//! [`set_memory_base_dir_override`].
//!
//! **Unified-runtime shape.** Unlike `kv/*` (whole-store rewrites), a memory
//! mutation is an append of a few encoded lines, so the `MemoryThread` itself
//! NEVER leaves the VM thread — only a [`FlushJob`] (path + tail lines) crosses
//! to the blocking tier:
//!
//! - `memory/open` offloads its initial read+parse as a plain External wait
//!   (`external_io_interruptible_try`, `kv/open`'s shape). Reopening a live
//!   thread is idempotent and never dispatches. If two opens of the same key
//!   race, the first insert wins and the loser's load is discarded — appends
//!   made meanwhile are never clobbered. The derived sidecar path is checked
//!   against the sandbox's allowed paths at open (the kv rule: open admits the
//!   resource; later writes trust the admission).
//! - `memory/append` pushes onto the working set on the VM thread (immediately
//!   visible to `memory/messages`), then offloads the tail flush through a
//!   per-thread FIFO [`ResourceGateHandle`] CHECKOUT (`sqlite.rs`'s canonical
//!   pattern). The call does not resolve until the flush lands — durability is
//!   write-through; only the wait moves off the VM thread. `unflushed` counts
//!   the un-persisted tail; a sibling append during a flight simply grows it.
//! - **Flush integrity.** A flush builds its whole tail into one buffer and, on
//!   ANY write error, truncates the sidecar back to its pre-write length — a
//!   partial flush (ENOSPC mid-tail) never leaves duplicated or torn lines for
//!   the retry to mash. In-flight accounting is generation-guarded: `take`
//!   records the snapshot boundary (`snapshot_len`) and flight count on the
//!   thread; `decode` decrements `unflushed` only when the thread generation
//!   still matches (a reopened thread ignores a stale flight's decode); disjoint
//!   snapshots mean even two racing flights never write the same line twice.
//! - `memory/messages` and the `agent/run {:memory h}` callbacks are pure
//!   working-set reads/pushes — never busy, never parked. The agent writeback
//!   runs inside `__agent-finish` (including a CANCELLED run's unwind, where
//!   parking is impossible), so inside a quantum it only marks the tail
//!   unflushed; the next `memory/append` or the interpreter-teardown flush
//!   picks it up.
//! - A mid-flush cancel tombstones the slot (the write completes unattended —
//!   disk truth is ambiguous, so the working set is retired rather than risking
//!   silent duplication) — but turns pushed AFTER the in-flight snapshot (e.g.
//!   an agent writeback that landed during the flight) are preserved in the
//!   tombstone and re-installed when `memory/open` recovers the thread by
//!   reloading disk truth (memory has no `close` builtin; open IS the recovery
//!   path). The teardown flush persists a tombstone's preserved turns too.
//!
//! **Bounds.** `memory/open` preflights the sidecar's size (a metadata stat)
//! and reads through a capped `Read::take`; `memory/append` rejects an
//! oversized turn pre-dispatch with the thread untouched. These caps — not a
//! wall-clock timer — are the finite-work bound for the uninterruptible flush
//! write (R09B `QUARANTINED-BOUNDED`).
//!
//! **Known limitations (accepted, documented):** on a case-insensitive
//! filesystem (default APFS/NTFS) two ids differing only by case are distinct
//! threads sharing one physical sidecar — don't do that. A `memory/append`
//! cancelled while parked at the flush gate reports the cancellation, but its
//! turn was already pushed and remains in the working set (a later flush
//! persists it): the push-then-flush split is what keeps `memory/messages`
//! and the agent seam non-blocking.
//!
//! At top level (no scheduler) every builtin keeps the synchronous shape.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::rc::Rc;

use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult, ResourceGateHandle};
use sema_core::{
    in_runtime_quantum, Caps, Conversation, Message, OptionsExt, Role, SemaError, Value,
};

use crate::runtime_offload::{checkout_external, external_io_interruptible_try, CheckoutOp};

/// Completion-kind tag for `memory/*` external waits ("me\0\0").
const MEMORY_COMPLETION_KIND: u64 = 0x6d65_0000;

/// Hard ceiling on the sidecar bytes `memory/open` will load.
const MEMORY_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Hard ceiling on one appended turn's encoded size (role + content).
const MEMORY_MAX_TURN_BYTES: usize = 16 * 1024 * 1024;

/// The caps applied to a thread, captured on the VM thread before any blocking
/// work dispatches.
#[derive(Clone, Copy)]
struct MemoryBounds {
    max_file_bytes: u64,
    max_turn_bytes: usize,
}

const MEMORY_RUNTIME_BOUNDS: MemoryBounds = MemoryBounds {
    max_file_bytes: MEMORY_MAX_FILE_BYTES,
    max_turn_bytes: MEMORY_MAX_TURN_BYTES,
};

/// One durable conversation turn. The `role` string is stored as given;
/// normalization to `Role` happens on read (`"assistant"` stays assistant,
/// anything else reads back as user — the documented rule).
#[derive(Debug, Clone)]
struct StoredTurn {
    role: String,
    content: String,
}

/// A live memory thread: its sidecar path plus the in-process working set.
struct MemoryThread {
    path: PathBuf,
    messages: Vec<StoredTurn>,
    /// Length of the tail of `messages` not yet durably on disk.
    unflushed: usize,
    /// Identity of this working set. A reopened (tombstone-recovered) thread
    /// gets a fresh generation, so a stale in-flight flush's `decode` — keyed
    /// by name — cannot corrupt the new thread's accounting.
    generation: u64,
    /// `Some(n)`: one or more flushes are in flight and their snapshots cover
    /// `messages[..n]`. A newer concurrent flight snapshots only `[n..]`, so
    /// racing flights write disjoint line ranges. Cleared when `flights` drops
    /// to zero.
    snapshot_len: Option<usize>,
    /// Number of offloaded flushes currently in flight for this thread. The
    /// sync flush (top level / teardown) skips while non-zero — it cannot know
    /// which lines the worker is mid-writing.
    flights: u32,
}

/// A registry slot. `Tombstone` is set only when a flush is cancelled
/// mid-flight (the write completes unattended — disk truth is ambiguous, so
/// the working set is retired rather than risking silent duplication).
/// `preserved` carries the turns pushed AFTER the in-flight snapshot — they are
/// covered by no write, so `memory/open` re-installs them (and the teardown
/// flush persists them) instead of silently dropping e.g. an agent writeback
/// that landed during the flight.
enum MemSlot {
    Live(MemoryThread),
    Tombstone {
        msg: String,
        path: PathBuf,
        preserved: Vec<StoredTurn>,
    },
}

/// The unflushed tail extracted for one offloaded flush: everything the
/// blocking tier needs, so the `MemoryThread` itself stays on the VM thread.
struct FlushJob {
    path: PathBuf,
    lines: Vec<String>,
    count: usize,
    /// The thread generation at snapshot time; `decode`/`reinstall` are no-ops
    /// when the registry thread no longer matches.
    generation: u64,
    /// Fault-injection seam value captured on the VM thread pre-dispatch.
    fail_after: Option<usize>,
}

thread_local! {
    /// Open threads, keyed `(namespace, id)`.
    static MEMORY_THREADS: RefCell<HashMap<(String, String), MemSlot>> =
        RefCell::new(HashMap::new());
    /// Per-thread owning flush gate (FIFO mutual exclusion for sidecar
    /// appends), created lazily on the first offloaded flush.
    static MEMORY_GATES: RefCell<HashMap<(String, String), ResourceGateHandle>> =
        RefCell::new(HashMap::new());
    /// Monotonic source for [`MemoryThread::generation`].
    static MEMORY_GENERATION: Cell<u64> = const { Cell::new(0) };
    /// Test seam: overrides the sidecar base dir (normally `./.sema/memory`).
    static MEMORY_BASE_DIR_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Test seam: lowered caps (clamped to the hard ceilings, never raised).
    static MEMORY_BOUNDS_OVERRIDE: Cell<Option<MemoryBounds>> = const { Cell::new(None) };
    /// Test seam: make the NEXT flush write only this many bytes and then fail
    /// (simulates ENOSPC mid-tail). One-shot; captured pre-dispatch.
    static MEMORY_FLUSH_PARTIAL_FAIL: Cell<Option<usize>> = const { Cell::new(None) };
    /// Whether this thread's interpreter has the teardown hook wired (C6).
    static MEMORY_TEARDOWN_REGISTERED: Cell<bool> = const { Cell::new(false) };
}

/// Test seam: point the sidecar base dir somewhere hermetic (kv-bounds-override
/// pattern). `None` restores the default `./.sema/memory`.
pub fn set_memory_base_dir_override(dir: Option<PathBuf>) {
    MEMORY_BASE_DIR_OVERRIDE.with(|c| *c.borrow_mut() = dir);
}

/// Lower the per-thread `(max_file_bytes, max_turn_bytes)` caps (clamped to
/// the hard ceilings), or clear the override with `None`.
pub fn set_memory_bounds_override(bounds: Option<(u64, usize)>) {
    MEMORY_BOUNDS_OVERRIDE.with(|cell| {
        cell.set(bounds.map(|(max_file_bytes, max_turn_bytes)| MemoryBounds {
            max_file_bytes,
            max_turn_bytes,
        }));
    });
}

/// Test seam: the NEXT flush writes only `n` bytes of its buffer and then
/// fails, exercising the partial-write rollback. One-shot.
pub fn set_memory_flush_partial_fail_override(bytes: Option<usize>) {
    MEMORY_FLUSH_PARTIAL_FAIL.with(|c| c.set(bytes));
}

/// The effective caps: the hard ceilings, lowered by any override.
fn effective_bounds() -> MemoryBounds {
    MEMORY_BOUNDS_OVERRIDE
        .with(Cell::get)
        .map_or(MEMORY_RUNTIME_BOUNDS, |over| MemoryBounds {
            max_file_bytes: over
                .max_file_bytes
                .min(MEMORY_RUNTIME_BOUNDS.max_file_bytes),
            max_turn_bytes: over
                .max_turn_bytes
                .min(MEMORY_RUNTIME_BOUNDS.max_turn_bytes),
        })
}

/// A fresh working-set generation.
fn next_generation() -> u64 {
    MEMORY_GENERATION.with(|g| {
        let next = g.get() + 1;
        g.set(next);
        next
    })
}

/// Interpreter-drop teardown (C6): flush any unflushed tails (bounded — a tail
/// accrues only between an agent writeback and the next flush point) and any
/// tombstone-preserved turns, then drop the registry and close the gates so
/// parked waiters fail fast. Flush errors are unreportable in a drop hook —
/// this is the best-effort end of the durability story, same as any buffered
/// writer at exit.
fn teardown_threads() {
    MEMORY_THREADS.with(|t| {
        for slot in t.borrow_mut().values_mut() {
            match slot {
                MemSlot::Live(thread) => {
                    let _ = flush_tail(thread);
                }
                MemSlot::Tombstone {
                    path, preserved, ..
                } => {
                    if !preserved.is_empty() {
                        let lines: Vec<String> = preserved.iter().map(encode_turn).collect();
                        if write_lines(path, &lines, None).is_ok() {
                            preserved.clear();
                        }
                    }
                }
            }
        }
        t.borrow_mut().clear();
    });
    MEMORY_GATES.with(|g| {
        for (_, gate) in g.borrow_mut().drain() {
            let _ = gate.close();
        }
    });
    MEMORY_TEARDOWN_REGISTERED.with(|c| c.set(false));
}

/// Register the teardown hook against `ctx` once per interpreter, at first open.
fn ensure_teardown_hook(ctx: &sema_core::EvalContext) {
    if !MEMORY_TEARDOWN_REGISTERED.with(|c| c.replace(true)) {
        ctx.register_interpreter_teardown_hook(teardown_threads);
    }
}

/// The sidecar base dir: the test override, else `./.sema/memory`.
fn base_dir() -> PathBuf {
    MEMORY_BASE_DIR_OVERRIDE
        .with(|c| c.borrow().clone())
        .unwrap_or_else(|| PathBuf::from("./.sema/memory"))
}

/// Validate a `(namespace, id)` path segment: these become file names, never
/// path expressions. Rejects separators and dot-dot; also `:` (a Windows drive
/// prefix like `C:notes` re-roots `Path::join`, and NTFS treats it as an
/// alternate-data-stream marker), control characters, and trailing dots/spaces
/// (silently stripped by Win32, colliding distinct ids onto one file).
fn check_segment(what: &str, s: &str) -> Result<(), SemaError> {
    let invalid = s.is_empty()
        || s == "."
        || s == ".."
        || s.contains(['/', '\\', ':'])
        || s.chars().any(char::is_control)
        || s.ends_with(['.', ' ']);
    if invalid {
        return Err(SemaError::eval(format!(
            "memory/open: invalid {what} {s:?} — must be a plain name (no path separators, \
             colons, control characters, or trailing dots/spaces)"
        )));
    }
    Ok(())
}

/// `<base>/<namespace>/<id>.jsonl`.
fn thread_path(namespace: &str, id: &str) -> PathBuf {
    base_dir().join(namespace).join(format!("{id}.jsonl"))
}

/// The handle map `{:memory/id id :memory/namespace ns}` all `memory/*`
/// functions accept.
fn handle_value(id: &str, namespace: &str) -> Value {
    let mut map = std::collections::BTreeMap::new();
    map.insert(Value::keyword("memory/id"), Value::string(id));
    map.insert(Value::keyword("memory/namespace"), Value::string(namespace));
    Value::map(map)
}

/// Read `(namespace, id)` back out of a handle map.
fn parse_handle(fn_name: &str, v: &Value) -> Result<(String, String), SemaError> {
    match (v.opt_str("memory/id"), v.opt_str("memory/namespace")) {
        (Some(id), Some(ns)) => Ok((ns, id)),
        _ => Err(SemaError::type_error(
            format!("{fn_name}: a memory handle (from memory/open)"),
            v.type_name(),
        )),
    }
}

/// Encode one turn as its sidecar JSONL line (no trailing newline).
fn encode_turn(turn: &StoredTurn) -> String {
    serde_json::json!({ "role": turn.role, "content": turn.content }).to_string()
}

/// Decode one sidecar line. Unknown fields are ignored; a missing role
/// defaults to `"user"` (same rule as `memory/append`).
fn decode_turn(line: &str) -> Option<StoredTurn> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let obj = v.as_object()?;
    Some(StoredTurn {
        role: obj
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("user")
            .to_string(),
        content: obj
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// Read a sidecar into turns through a capped reader; runs on whichever thread
/// owns the read (worker under a quantum, VM thread at top level). A missing
/// file is an empty thread, not an error.
fn read_sidecar(path: &PathBuf, bounds: MemoryBounds) -> Result<Vec<StoredTurn>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let mut text = String::new();
    let mut capped = file.take(bounds.max_file_bytes.saturating_add(1));
    capped
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if text.len() as u64 > bounds.max_file_bytes {
        return Err(format!(
            "sidecar {} exceeds the {} byte cap",
            path.display(),
            bounds.max_file_bytes
        ));
    }
    Ok(text.lines().filter_map(decode_turn).collect())
}

/// Synchronously flush the unflushed tail (top level / teardown). Skips while
/// an offloaded flush is in flight — the worker owns the file, and its lines
/// overlap this tail; the accounting settles when it lands.
fn flush_tail(thread: &mut MemoryThread) -> Result<(), SemaError> {
    if thread.unflushed == 0 || thread.flights > 0 {
        return Ok(());
    }
    let tail_start = thread.messages.len() - thread.unflushed;
    let lines: Vec<String> = thread.messages[tail_start..]
        .iter()
        .map(encode_turn)
        .collect();
    let fail_after = MEMORY_FLUSH_PARTIAL_FAIL.with(Cell::take);
    write_lines(&thread.path, &lines, fail_after).map_err(SemaError::eval)?;
    thread.unflushed = 0;
    Ok(())
}

/// The blocking append shared by the sync flush and the offloaded
/// [`FlushJob`]: one buffer, one `write_all`, and on ANY write error the file
/// is truncated back to its pre-write length so a partial tail (ENOSPC
/// mid-write) can never duplicate on retry or tear the next line.
fn write_lines(path: &PathBuf, lines: &[String], fail_after: Option<usize>) -> Result<(), String> {
    let io_err = |e: std::io::Error| format!("memory/append: cannot write {}: {e}", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(io_err)?;
    let pre_len = file.metadata().map_err(io_err)?.len();
    let mut buf = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    for line in lines {
        buf.push_str(line);
        buf.push('\n');
    }
    let write_result = match fail_after {
        // Fault-injection seam: land a partial prefix, then report failure —
        // exactly what a mid-tail ENOSPC leaves behind.
        Some(n) => {
            let cut = n.min(buf.len());
            let _ = file.write_all(&buf.as_bytes()[..cut]);
            Err("injected flush failure (memory test seam)".to_string())
        }
        None => file.write_all(buf.as_bytes()).map_err(io_err),
    };
    if let Err(e) = write_result {
        // Roll the partial append back via a fresh write-mode handle: the
        // append-mode handle lacks FILE_WRITE_DATA on Windows, so set_len on
        // it fails silently and the torn line would survive. If even this
        // fails the disk is gone anyway and the retry-duplication hazard is
        // moot.
        drop(file);
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = f.set_len(pre_len);
        }
        return Err(e);
    }
    Ok(())
}

/// Run `f` over the LIVE thread for `handle`, with clear errors for a
/// never-opened or tombstoned slot.
fn with_thread<R>(
    fn_name: &str,
    handle: &Value,
    f: impl FnOnce(&mut MemoryThread) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    let (ns, id) = parse_handle(fn_name, handle)?;
    with_thread_key(fn_name, &(ns, id), f)
}

/// [`with_thread`] over an already-parsed key.
fn with_thread_key<R>(
    fn_name: &str,
    key: &(String, String),
    f: impl FnOnce(&mut MemoryThread) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    let (ns, id) = key;
    MEMORY_THREADS.with(
        |t| match t.borrow_mut().get_mut(&(ns.clone(), id.clone())) {
            Some(MemSlot::Live(thread)) => f(thread),
            Some(MemSlot::Tombstone { msg, .. }) => Err(SemaError::eval(format!(
                "{fn_name}: memory thread {ns}/{id} was retired ({msg})"
            ))
            .with_hint("reopen it with memory/open to reload the sidecar")),
            None => Err(
                SemaError::eval(format!("{fn_name}: no open memory thread {ns}/{id}"))
                    .with_hint("open it first with (memory/open {:id ... :namespace ...})"),
            ),
        },
    )
}

/// Normalize a stored role string to a `Role` (documented rule: `"assistant"`
/// stays assistant, everything else reads back as user).
fn normalize_role(role: &str) -> Role {
    if role == "assistant" {
        Role::Assistant
    } else {
        Role::User
    }
}

/// Install a freshly-loaded working set for `key` — disk turns plus any
/// tombstone-preserved tail (which is not yet durable, so it re-enters as
/// unflushed) — unless a live thread already exists (two racing opens: the
/// first insert wins; the loser's load is discarded so sibling appends are
/// never clobbered).
fn install_loaded(
    key: (String, String),
    path: PathBuf,
    turns: Vec<StoredTurn>,
    preserved: Vec<StoredTurn>,
) {
    MEMORY_THREADS.with(|t| {
        let mut threads = t.borrow_mut();
        if !matches!(threads.get(&key), Some(MemSlot::Live(_))) {
            let mut messages = turns;
            let unflushed = preserved.len();
            messages.extend(preserved);
            threads.insert(
                key,
                MemSlot::Live(MemoryThread {
                    path,
                    messages,
                    unflushed,
                    generation: next_generation(),
                    snapshot_len: None,
                    flights: 0,
                }),
            );
        }
    });
}

/// `(memory/open {:id id :namespace ns})` → handle. Idempotent per process;
/// recovers a tombstoned slot by reloading disk truth and re-installing the
/// tombstone's preserved (never-written) turns.
fn memory_open(
    sandbox: &sema_core::Sandbox,
    ctx: &sema_core::EvalContext,
    args: &[Value],
) -> NativeResult {
    sema_core::check_arity!(args, "memory/open", 1);
    let opts = &args[0];
    if opts.as_map_rc().is_none() {
        return Err(SemaError::type_error(
            "memory/open: an options map {:id ... :namespace ...}",
            opts.type_name(),
        ));
    }
    let id = opts
        .opt_str("id")
        .ok_or_else(|| SemaError::eval("memory/open: :id (string) is required"))?;
    let namespace = opts
        .opt_str("namespace")
        .unwrap_or_else(|| "default".to_string());
    check_segment(":id", &id)?;
    check_segment(":namespace", &namespace)?;

    ensure_teardown_hook(ctx);
    let key = (namespace.clone(), id.clone());
    let already_live =
        MEMORY_THREADS.with(|t| matches!(t.borrow().get(&key), Some(MemSlot::Live(_))));
    if already_live {
        return Ok(NativeOutcome::Return(handle_value(&id, &namespace)));
    }

    let path = thread_path(&namespace, &id);
    // The path is derived, not an argument, so the registration helper cannot
    // check it — admit it against the sandbox's allowed paths here (open-time
    // admission; later writes trust it, the kv rule).
    sandbox.check_path(&path.to_string_lossy(), "memory/open")?;
    let bounds = effective_bounds();
    // Preflight the sidecar size on the VM thread (a metadata stat) so an
    // oversized file is rejected before any blocking work dispatches; the
    // capped read re-checks against a racing grow.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > bounds.max_file_bytes {
            return Err(SemaError::eval(format!(
                "memory/open: sidecar {} exceeds the {} byte cap",
                path.display(),
                bounds.max_file_bytes
            )));
        }
    }

    // A tombstoned slot's preserved tail rides along into the reinstall (read,
    // not taken: racing opens then carry identical copies and the first insert
    // wins as usual).
    let preserved = MEMORY_THREADS.with(|t| match t.borrow().get(&key) {
        Some(MemSlot::Tombstone { preserved, .. }) => preserved.clone(),
        _ => Vec::new(),
    });

    if in_runtime_quantum() {
        let kind = CompletionKind::try_from_raw(MEMORY_COMPLETION_KIND)
            .expect("memory completion kind is nonzero");
        let handle = handle_value(&id, &namespace);
        let worker_path = path.clone();
        return external_io_interruptible_try(
            "memory/open",
            kind,
            "memory sidecar load",
            move |turns: Vec<StoredTurn>| {
                install_loaded(key, path, turns, preserved);
                Ok(handle)
            },
            move || async move { read_sidecar(&worker_path, bounds) },
        );
    }

    let turns = read_sidecar(&path, bounds).map_err(SemaError::eval)?;
    install_loaded(key, path, turns, preserved);
    Ok(NativeOutcome::Return(handle_value(&id, &namespace)))
}

/// Offload the current unflushed tail through the thread's FIFO flush gate.
/// The working-set push already happened on the VM thread; this parks the
/// caller until the tail is durable.
fn checkout_flush(key: (String, String)) -> NativeResult {
    let kind = CompletionKind::try_from_raw(MEMORY_COMPLETION_KIND)
        .expect("memory completion kind is nonzero");
    let gate = MEMORY_GATES.with(|g| g.borrow().get(&key).cloned());
    let k_take = key.clone();
    let k_decode = key.clone();
    let k_reinstall = key.clone();
    let k_tomb = key.clone();
    let k_remove = key.clone();
    let k_store = key;
    checkout_external(CheckoutOp {
        op_name: "memory/append",
        kind,
        gate,
        store_gate: Box::new(move |id| {
            MEMORY_GATES.with(|g| {
                g.borrow_mut().insert(k_store, id);
            });
        }),
        remove_gate: Rc::new(move |id| {
            MEMORY_GATES.with(|g| {
                let mut gates = g.borrow_mut();
                if gates.get(&k_remove).map(ResourceGateHandle::id) == Some(id) {
                    gates.remove(&k_remove);
                }
            });
        }),
        // Snapshot the tail on the VM thread once the gate is owned; the
        // thread itself stays in the registry (reads/pushes never block). The
        // snapshot starts past any in-flight snapshot's boundary, so even two
        // racing flights (a forked-gate window) cover disjoint line ranges.
        take: Box::new(move || {
            MEMORY_THREADS.with(|t| match t.borrow_mut().get_mut(&k_take) {
                Some(MemSlot::Live(thread)) => {
                    let unflushed_start = thread.messages.len() - thread.unflushed;
                    let tail_start = unflushed_start.max(thread.snapshot_len.unwrap_or(0));
                    let lines: Vec<String> = thread.messages[tail_start..]
                        .iter()
                        .map(encode_turn)
                        .collect();
                    let count = lines.len();
                    thread.snapshot_len = Some(thread.messages.len());
                    thread.flights += 1;
                    Ok(FlushJob {
                        path: thread.path.clone(),
                        lines,
                        count,
                        generation: thread.generation,
                        fail_after: MEMORY_FLUSH_PARTIAL_FAIL.with(Cell::take),
                    })
                }
                Some(MemSlot::Tombstone { msg, .. }) => Err(SemaError::eval(format!(
                    "memory/append: memory thread {}/{} was retired ({msg})",
                    k_take.0, k_take.1
                ))),
                None => Err(SemaError::eval(format!(
                    "memory/append: no open memory thread {}/{}",
                    k_take.0, k_take.1
                ))),
            })
        }),
        op: Box::new(|job: &mut FlushJob| {
            // A predecessor's snapshot may already have covered this whole
            // tail; a zero-line job is a durable no-op.
            if job.lines.is_empty() {
                return Ok((0, job.generation));
            }
            write_lines(&job.path, &job.lines, job.fail_after).map(|()| (job.count, job.generation))
        }),
        // Runs on success AND on a recoverable op error: the flight is over
        // either way. Generation-guarded so a stale flight cannot touch a
        // reopened thread's state.
        reinstall: Box::new(move |job: FlushJob| {
            MEMORY_THREADS.with(|t| {
                if let Some(MemSlot::Live(thread)) = t.borrow_mut().get_mut(&k_reinstall) {
                    if thread.generation == job.generation {
                        thread.flights = thread.flights.saturating_sub(1);
                        if thread.flights == 0 {
                            thread.snapshot_len = None;
                        }
                    }
                }
            });
        }),
        // The flushed-tail accounting lives here, so `success_value` must stay
        // `None` (a set success value SKIPS decode entirely). The returned
        // handle is rebuilt from the key strings — decode must not capture a
        // `Value` (it is not traced).
        decode: Box::new(move |(count, generation): (usize, u64)| {
            MEMORY_THREADS.with(|t| {
                if let Some(MemSlot::Live(thread)) = t.borrow_mut().get_mut(&k_decode) {
                    if thread.generation == generation {
                        thread.unflushed = thread.unflushed.saturating_sub(count);
                    }
                }
            });
            Ok(handle_value(&k_decode.1, &k_decode.0))
        }),
        success_value: None,
        // A mid-flush cancel retires the working set (disk truth is ambiguous),
        // but turns pushed after the in-flight snapshot are covered by no write
        // — carry them in the tombstone for `memory/open` to re-install.
        tombstone: Rc::new(move |msg| {
            MEMORY_THREADS.with(|t| {
                let mut threads = t.borrow_mut();
                let (path, preserved) = match threads.get(&k_tomb) {
                    Some(MemSlot::Live(thread)) => (
                        thread.path.clone(),
                        thread.messages[thread.snapshot_len.unwrap_or(thread.messages.len())..]
                            .to_vec(),
                    ),
                    Some(MemSlot::Tombstone {
                        path, preserved, ..
                    }) => (path.clone(), preserved.clone()),
                    None => (thread_path(&k_tomb.0, &k_tomb.1), Vec::new()),
                };
                threads.insert(
                    k_tomb.clone(),
                    MemSlot::Tombstone {
                        msg,
                        path,
                        preserved,
                    },
                );
            });
        }),
        abort: None,
        reclaim: None,
        terminal_on_success: false,
    })
}

/// `(memory/append handle {:role r :content text})` → handle (chainable).
fn memory_append(args: &[Value]) -> NativeResult {
    sema_core::check_arity!(args, "memory/append", 2);
    let msg = &args[1];
    if msg.as_map_rc().is_none() {
        return Err(SemaError::type_error(
            "memory/append: a message map {:role ... :content ...}",
            msg.type_name(),
        ));
    }
    let role = msg.opt_str("role").unwrap_or_else(|| "user".to_string());
    let content = msg
        .opt_str("content")
        .ok_or_else(|| SemaError::eval("memory/append: :content (string) is required"))?;
    let bounds = effective_bounds();
    if role.len() + content.len() > bounds.max_turn_bytes {
        return Err(SemaError::eval(format!(
            "memory/append: turn of {} bytes exceeds the {} byte cap",
            role.len() + content.len(),
            bounds.max_turn_bytes
        )));
    }

    // Push on the VM thread — immediately visible to `memory/messages` — then
    // make it durable: offloaded through the flush gate inside a quantum,
    // synchronously at top level.
    let key = parse_handle("memory/append", &args[0])?;
    with_thread_key("memory/append", &key, |thread| {
        thread.messages.push(StoredTurn { role, content });
        thread.unflushed += 1;
        Ok(())
    })?;

    if in_runtime_quantum() {
        return checkout_flush(key);
    }
    with_thread_key("memory/append", &key, flush_tail)?;
    Ok(NativeOutcome::Return(args[0].clone()))
}

/// `(memory/messages handle)` → `Conversation` over the working set.
fn memory_messages(args: &[Value]) -> Result<Value, SemaError> {
    sema_core::check_arity!(args, "memory/messages", 1);
    let messages = with_thread("memory/messages", &args[0], |thread| {
        Ok(thread
            .messages
            .iter()
            .map(|turn| Message {
                role: normalize_role(&turn.role),
                content: turn.content.clone(),
                images: Vec::new(),
            })
            .collect::<Vec<_>>())
    })?;
    Ok(Value::conversation(Conversation {
        messages,
        model: String::new(),
        metadata: std::collections::BTreeMap::new(),
    }))
}

/// `agent/run {:memory h}` seed: the thread's working set as plain
/// `ChatMessage`s (roles as stored). Text turns only, so a seeded history is
/// always provider-valid — no dangling tool correlation can enter a request.
fn agent_get_working(handle: &Value) -> Result<Vec<sema_llm::types::ChatMessage>, SemaError> {
    with_thread("agent/run :memory", handle, |thread| {
        Ok(thread
            .messages
            .iter()
            .map(|turn| sema_llm::types::ChatMessage::new(&turn.role, &turn.content))
            .collect())
    })
}

/// `agent/run {:memory h}` writeback: append the run's new turns to the
/// thread's TEXT transcript — tool-protocol messages (`role == "tool"`, or
/// assistant `tool_calls` turns with no text) are wire detail, not
/// conversation, and are skipped. Runs on the VM quantum — including a
/// cancelled run's unwind — so it must not block or park: inside a quantum the
/// sidecar flush is deferred (picked up by the next `memory/append` or the
/// interpreter-teardown flush); at top level it flushes synchronously.
fn agent_append_back(
    handle: &Value,
    turns: &[sema_llm::types::ChatMessage],
) -> Result<(), SemaError> {
    with_thread("agent/run :memory", handle, |thread| {
        for msg in turns {
            if msg.role == "tool" {
                continue;
            }
            let content = msg.content.to_text();
            if content.is_empty() {
                continue;
            }
            thread.messages.push(StoredTurn {
                role: msg.role.clone(),
                content,
            });
            thread.unflushed += 1;
        }
        if in_runtime_quantum() {
            Ok(())
        } else {
            flush_tail(thread)
        }
    })
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // FS_WRITE, not FS_READ: open mints a persistent read-write resource —
    // appends, the agent writeback, and the teardown flush all write its
    // sidecar (kv/open's rule). The body also gets the sandbox for the
    // derived-path admission check.
    let open_sandbox = sandbox.clone();
    crate::register_runtime_fn_path_gated_ctx(
        env,
        sandbox,
        Caps::FS_WRITE,
        "memory/open",
        &[],
        move |ctx, args| memory_open(&open_sandbox, ctx, args),
    );
    crate::register_runtime_fn_gated(env, sandbox, Caps::FS_WRITE, "memory/append", memory_append);
    // Pure in-memory read — no capability needed.
    crate::register_fn(env, "memory/messages", memory_messages);
    sema_llm::builtins::register_memory_callbacks(agent_get_working, agent_append_back);
}
