//! The append-only JSONL journal — the system of record for a workflow run.
//!
//! Every actual filesystem write happens OFF the VM thread on a per-run
//! [`JournalWriter`] thread (see `writer.rs`): the VM thread only opens/claims the run
//! directory + `events.jsonl` (bounded pre-dispatch admission) and then renders each
//! event/memo/sidecar to a `String`/JSON and `try_send`s it to the writer. So this
//! module holds the writer HANDLE, not a `BufWriter` — no `write_all`/`fs::write` runs
//! on the quantum. The frozen append-only journal contract is unchanged: one
//! `serde_json` line + `'\n'` per event, flushed per event (a crash mid-run leaves a
//! readable prefix), and write errors swallowed (a journal hiccup must never crash the
//! run — same trust model as the OTel file exporter).
//!
//! Rust-side I/O here BYPASSES the Sema VFS sandbox, exactly like the OTel file
//! exporter. That is intentional and the same trust model: the run directory is an
//! operator-facing artifact, not script-controlled state.
//!
//! Run-dir layout (FROZEN public contract — mirror of `.semac`):
//! ```text
//! .sema/runs/<run-id>/
//!   events.jsonl              # append-only, the system of record
//!   events.resume-<n>.jsonl   # one per --resume continuation (each keeps the invariants)
//!   memo/<content-key>.json   # per-leaf memoized values (the resume source of truth)
//!   args.json                 # the --args input, verbatim
//!   metadata.json             # workflow name, code version, budget, permissions
//!   result.json               # the final {:status …} envelope
//! ```

use std::fs;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::event::WorkflowEvent;
use crate::writer::JournalWriter;

/// Bounded wait for the host (non-quantum) terminal flush-ack and for unit tests: a
/// runtime quantum instead parks on the External flush-ack (see `workflow/run`'s
/// `finish_run`), never this blocking wait.
const HOST_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// One run's journal. Owns the run dir path and the bounded writer HANDLE. Every write
/// method enqueues onto the writer thread; no filesystem write runs here.
pub struct Journal {
    dir: PathBuf,
    writer: JournalWriter,
}

impl Journal {
    /// Create a FRESH run and open its `events.jsonl`, handing the open file to a
    /// dedicated writer thread. The journal file is claimed ATOMICALLY with `create_new`,
    /// so a fresh run whose journal already exists fails with [`io::ErrorKind::AlreadyExists`]
    /// rather than appending to (and corrupting) another run's frozen event stream — the A2
    /// run-identity guarantee. The open/claim error propagates so the runtime can fail the
    /// run cleanly (per-event enqueues below are best-effort).
    ///
    /// `runs_root` is normally [`crate::RUNS_ROOT`] (project-local `.sema/runs`),
    /// resolved cwd-relative — NOT `~/.sema`.
    pub fn open(runs_root: impl AsRef<Path>, run_id: &str) -> io::Result<Self> {
        Self::open_named(runs_root, run_id, "events.jsonl")
    }

    /// As [`Self::open`] but writes to a named events file. A resume does NOT come through
    /// here — it claims a sibling `events.resume-<n>.jsonl` segment via
    /// [`next_resume_segment`] (each segment keeps the frozen invariants: first line
    /// `run.started`, `seq` monotonic from 0; the reader merges segments).
    ///
    /// The run DIRECTORY may already exist without being a collision — a `:persist :run`
    /// MCP credential store is seeded under it before the run opens (see
    /// [`ensure_run_dir`]); the exclusive claim is the journal file's `create_new`, not
    /// the directory, so two runs never share an event stream even if they share the dir.
    pub fn open_named(
        runs_root: impl AsRef<Path>,
        run_id: &str,
        filename: &str,
    ) -> io::Result<Self> {
        let dir = runs_root.as_ref().join(run_id);
        ensure_run_dir(&dir)?;
        let path = dir.join(filename);
        let file = OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok(Self::from_open(dir, file))
    }

    /// Wrap an already-opened `events.jsonl` handle + its run dir with a fresh writer
    /// thread. Shared by [`Self::open_named`] and [`next_resume_segment`].
    fn from_open(dir: PathBuf, file: File) -> Self {
        let writer = JournalWriter::spawn(dir.clone(), file);
        Self { dir, writer }
    }

    /// Enqueue a per-leaf memo value to `memo/<content_key>.json`. The file's EXISTENCE
    /// is the resume source of truth: present ⇒ that leaf completed with this value, so a
    /// resumed run short-circuits it. The whole-file write happens on the writer thread.
    /// Best-effort by design; a dropped memo just means that leaf re-runs on resume.
    /// Cap enforcement (round-trip guard + size/count caps) lives in
    /// [`crate::context::WorkflowCtx::memo_store`], which decides whether to call this.
    pub fn write_memo(&self, content_key: &str, value: &serde_json::Value) {
        let mut json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        json.push('\n');
        self.writer.enqueue_memo(content_key.to_string(), json);
    }

    /// A throwaway journal that writes into a temp directory — for unit tests that need a
    /// `WorkflowCtx` but don't inspect the journal.
    #[doc(hidden)]
    pub fn null() -> Self {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "sema-wf-null-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // Best-effort: a failure here only affects tests, so fall back to a sink path.
        Self::open(&dir, "null").unwrap_or_else(|_| {
            let f = OpenOptions::new()
                .append(true)
                .create(true)
                .open(std::env::temp_dir().join("sema-wf-null.jsonl"))
                .expect("temp dir is writable for the null journal");
            Self::from_open(dir, f)
        })
    }

    /// The absolute-or-relative run directory this journal writes into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Enqueue a terminal flush barrier and return the ack receiver, WITHOUT waiting. The
    /// runtime (quantum) terminal path parks on this via an External wait so a normal
    /// `workflow/run` return means the journal is durable; the host path uses
    /// [`Self::flush_blocking`].
    pub fn request_flush(&self) -> Receiver<()> {
        self.writer.request_flush()
    }

    /// Block (bounded) until the writer has flushed every enqueued message to disk. For
    /// the host (non-quantum) terminal path and unit tests — NEVER call from inside a
    /// runtime quantum (park on the External flush-ack instead). Not a filesystem call:
    /// it waits for the writer thread's ack.
    pub fn flush_blocking(&self) {
        let ack = self.writer.request_flush();
        let _ = ack.recv_timeout(HOST_FLUSH_TIMEOUT);
    }

    /// Append one event as a single JSON line + `'\n'` (the writer thread appends the
    /// newline). Best-effort: a serialize failure renders a fallback error line; a full
    /// queue drops the line and surfaces one `journal.overflow` marker when space returns.
    pub fn write(&self, event: &WorkflowEvent) {
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => format!("{{\"error\":\"workflow journal serialize: {e}\"}}"),
        };
        self.writer.enqueue_event(line);
    }

    /// Enqueue `args.json` (the `--args` input, verbatim). Pretty-printed for human
    /// inspection; NOT part of the byte-identical events.jsonl oracle.
    pub fn write_args(&self, args: &serde_json::Value) {
        self.write_sidecar("args.json", args);
    }

    /// Enqueue `metadata.json` (workflow name, code version, budget, permissions).
    pub fn write_metadata(&self, metadata: &serde_json::Value) {
        self.write_sidecar("metadata.json", metadata);
    }

    /// Enqueue `result.json` (the final `{:status …}` envelope as JSON).
    pub fn write_result(&self, result: &serde_json::Value) {
        self.write_sidecar("result.json", result);
    }

    /// Enqueue a pretty-printed JSON sidecar write into the run dir. These are whole-file
    /// writes (truncate) on the writer thread, unlike the append-only events stream.
    fn write_sidecar(&self, name: &str, value: &serde_json::Value) {
        let mut s = serde_json::to_string_pretty(value)
            .unwrap_or_else(|e| format!("{{\"error\":\"workflow {name} serialize: {e}\"}}"));
        s.push('\n');
        self.writer.enqueue_sidecar(name.to_string(), s);
    }
}

/// Ensure the run directory exists so the journal file can be created inside it. The dir
/// may already exist WITHOUT being a run collision — a `:persist :run` MCP credential
/// store is seeded under `<run_dir>/<run_id>/auth/` before the run opens its journal — so
/// a pre-existing directory is adopted, not rejected. The exclusive per-run claim is the
/// journal file's `create_new` (in [`Journal::open_named`] / [`next_resume_segment`]), not
/// the directory. A recursive `create_dir_all` over the RUN dir is deliberately avoided:
/// only the parent chain is created recursively (the A2 source guard forbids the run-dir
/// reuse shape).
fn ensure_run_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::create_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Load all memo sidecars for a prior run into `(content_key, json)` pairs. Returns an
/// empty vec when there is no `memo/` dir (a fresh or never-memoized run). Best-effort:
/// an unreadable/corrupt memo file is skipped (that leaf re-runs), never fatal.
pub fn load_memos(runs_root: impl AsRef<Path>, run_id: &str) -> Vec<(String, serde_json::Value)> {
    let memo_dir = runs_root.as_ref().join(run_id).join("memo");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&memo_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                out.push((stem.to_string(), json));
            }
        }
    }
    out
}

/// Atomically claim the next free `events.resume-<n>.jsonl` segment under an EXISTING run
/// dir and return an opened journal writing to it (1-based; the first resume claims
/// `events.resume-1.jsonl`). The claim IS the open: `create_new` tests-and-takes the
/// segment in one syscall, so two concurrent resumes of the same run can never grab the
/// same file — there is no exists-probe TOCTOU window. The run dir must already exist (a
/// resume of a nonexistent run is rejected upstream in `set_workflow_scope`); this never
/// creates it.
pub fn next_resume_segment(runs_root: impl AsRef<Path>, run_id: &str) -> io::Result<Journal> {
    let dir = runs_root.as_ref().join(run_id);
    let mut n: u32 = 1;
    loop {
        let path = dir.join(format!("events.resume-{n}.jsonl"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                return Ok(Journal::from_open(dir, file));
            }
            // Segment already claimed (by us on a prior resume, or a racing resume) — try
            // the next ordinal. The claim above is atomic, so the loser here simply moves on.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                n = n.checked_add(1).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "resume segment ordinal space exhausted",
                    )
                })?;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "sema-wf-journal-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    #[test]
    fn writes_events_jsonl_one_line_per_event() {
        let root = tmp_root();
        let j = Journal::open(&root, "wf_test_0001").unwrap();
        j.write(&WorkflowEvent::RunStarted {
            seq: 0,
            ts: "0".into(),
            workflow: "hello-wf".into(),
            run_id: "wf_test_0001".into(),
            code_version: String::new(),
            args_json: String::new(),
            phases: Vec::new(),
        });
        j.write(&WorkflowEvent::RunEnded {
            seq: 1,
            ts: "0".into(),
            status: "success".into(),
            reason: None,
            dur_ms: 0,
        });
        j.flush_blocking(); // barrier: every event is on disk before we read
        drop(j);

        let body = fs::read_to_string(root.join("wf_test_0001").join("events.jsonl")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with(r#"{"event":"run.started""#));
        assert!(lines[1].starts_with(r#"{"event":"run.ended""#));
        assert!(body.ends_with('\n'));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sidecars_land_in_run_dir() {
        let root = tmp_root();
        let j = Journal::open(&root, "wf_test_0002").unwrap();
        j.write_args(&serde_json::json!({"name": "x"}));
        j.write_result(&serde_json::json!({"status": "success"}));
        j.flush_blocking();
        assert!(j.dir().join("args.json").exists());
        assert!(j.dir().join("result.json").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn open_fresh_fails_when_journal_already_exists() {
        // A fresh run must never append to an existing journal: the second open of the
        // same id fails with AlreadyExists (the create_new claim), leaving the first
        // run's events.jsonl untouched.
        let root = tmp_root();
        let _first = Journal::open(&root, "wf_dupe").unwrap();
        let err = match Journal::open(&root, "wf_dupe") {
            Ok(_) => panic!("second fresh open must fail"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn open_fresh_adopts_a_pre_existing_dir_without_a_journal() {
        // The run dir may already hold non-journal artifacts (e.g. a `:persist :run` MCP
        // credential store seeded before the run). With no events.jsonl present that is
        // NOT a collision — the fresh run adopts the dir and claims its journal.
        let root = tmp_root();
        fs::create_dir_all(root.join("wf_seeded").join("auth")).unwrap();
        let j = Journal::open(&root, "wf_seeded").expect("adopt a dir with no journal");
        assert!(j.dir().join("events.jsonl").exists());
        assert!(j.dir().join("auth").is_dir(), "seeded content is preserved");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn concurrent_resume_segment_claims_are_distinct() {
        use std::sync::{Arc, Barrier};

        // Two resumes of the SAME run racing on the segment claim must land on distinct
        // files: `create_new` makes each claim atomic, so exactly one thread wins each
        // ordinal — the pair ends up on events.resume-1 and events.resume-2, never both
        // on the same segment (the exists-probe TOCTOU this replaces could double-claim).
        let root = tmp_root();
        let run = "wf_race";
        fs::create_dir_all(root.join(run)).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let journal = next_resume_segment(&root, run).expect("segment claim succeeds");
                // Write one event so each claimed segment is a real, distinct file.
                journal.write(&WorkflowEvent::RunEnded {
                    seq: 0,
                    ts: "0".into(),
                    status: "success".into(),
                    reason: None,
                    dur_ms: 0,
                });
                journal.flush_blocking();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let mut names: Vec<String> = fs::read_dir(root.join(run))
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("events.resume-"))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "events.resume-1.jsonl".to_string(),
                "events.resume-2.jsonl".to_string()
            ],
            "concurrent resumes must claim two distinct segments"
        );
        fs::remove_dir_all(&root).ok();
    }
}
