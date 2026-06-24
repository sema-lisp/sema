//! The append-only JSONL journal — the system of record for a workflow run.
//!
//! Writer shape is copied from `sema_otel::file_exporter::JsonlFileExporter`:
//! `OpenOptions::new().append(true).create(true)`, a `BufWriter`, one
//! `serde_json` line + `'\n'` per event, **flush per event** (so a crash mid-run
//! still leaves a readable prefix), and **swallow write errors** (a journal write
//! failure must never crash the running workflow — same trust model and rationale
//! as the OTel exporter). Unlike that exporter's `span_to_json`, this journal
//! preserves the FULL event vocabulary — it does not drop events.
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
//!   metadata.json             # workflow name, code version, budget, perms
//!   result.json               # the final {:status …} envelope
//! ```

use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::event::WorkflowEvent;

/// One run's journal. Owns the open `events.jsonl` handle and knows the run dir so
/// the sidecar JSON files (`args.json`, `metadata.json`, `result.json`) land next
/// to it.
pub struct Journal {
    dir: PathBuf,
    writer: BufWriter<File>,
}

impl Journal {
    /// Create (or reuse) the run directory `<runs_root>/<run_id>/` and open its
    /// `events.jsonl` for append. Errors propagate to the caller so the workflow
    /// runtime can decide whether to abort (vs. the per-event writes below, which
    /// are best-effort and swallow failures).
    ///
    /// `runs_root` is normally [`crate::RUNS_ROOT`] (project-local `.sema/runs`),
    /// resolved cwd-relative — NOT `~/.sema`.
    pub fn open(runs_root: impl AsRef<Path>, run_id: &str) -> io::Result<Self> {
        Self::open_named(runs_root, run_id, "events.jsonl")
    }

    /// As [`Self::open`] but writes to a named events file under the run dir. Resume
    /// writes a sibling `events.resume-<n>.jsonl` segment so each file keeps the frozen
    /// invariants (first line is `run.started`, `seq` monotonic from 0) intact — the
    /// reader merges segments. See [`next_resume_segment`].
    pub fn open_named(
        runs_root: impl AsRef<Path>,
        run_id: &str,
        filename: &str,
    ) -> io::Result<Self> {
        let dir = runs_root.as_ref().join(run_id);
        fs::create_dir_all(&dir)?;
        let path = dir.join(filename);
        let file = OpenOptions::new().append(true).create(true).open(&path)?;
        Ok(Self {
            dir,
            writer: BufWriter::new(file),
        })
    }

    /// Write a per-leaf memo value to `memo/<content_key>.json`. The file's EXISTENCE
    /// is the resume source of truth: present ⇒ that leaf completed with this value, so
    /// a resumed run short-circuits it. Keeps the frozen `events.jsonl` untouched (the
    /// memo dir is a NEW best-effort sidecar, like `result.json`). Best-effort by
    /// design; a failed memo write just means that leaf re-runs on resume.
    pub fn write_memo(&self, content_key: &str, value: &serde_json::Value) -> io::Result<()> {
        let memo_dir = self.dir.join("memo");
        fs::create_dir_all(&memo_dir)?;
        let path = memo_dir.join(format!("{content_key}.json"));
        let mut s = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        s.push('\n');
        fs::write(path, s)
    }

    /// A throwaway journal that writes into a temp directory — for unit tests that
    /// need a `WorkflowCtx` but don't inspect the journal. Keeps the `BufWriter<File>`
    /// field shape (no `dyn Write` churn).
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
            Self {
                dir,
                writer: BufWriter::new(f),
            }
        })
    }

    /// The absolute-or-relative run directory this journal writes into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Best-effort flush of the buffered writer (e.g. at `run.ended`). Swallows
    /// errors, matching [`Self::write`].
    pub fn flush(&mut self) {
        let _ = self.writer.flush();
    }

    /// Append one event as a single JSON line + `'\n'`, flushing immediately.
    ///
    /// Best-effort: a serialize or write failure is swallowed (returns `Ok(())`) so a
    /// journaling hiccup cannot crash the workflow — matching the OTel exporter's
    /// "never crash the VM" contract. The on-disk prefix stays valid JSONL.
    pub fn write(&mut self, event: &WorkflowEvent) {
        let mut line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => format!("{{\"error\":\"workflow journal serialize: {e}\"}}"),
        };
        line.push('\n');
        // Swallow both the write and the flush error: journaling must not abort the run.
        let _ = self.writer.write_all(line.as_bytes());
        let _ = self.writer.flush();
    }

    /// Write `args.json` (the `--args` input, verbatim). Pretty-printed for human
    /// inspection; this file is NOT part of the byte-identical events.jsonl oracle.
    pub fn write_args(&self, args: &serde_json::Value) -> io::Result<()> {
        self.write_sidecar("args.json", args)
    }

    /// Write `metadata.json` (workflow name, code version, budget, perms). Caller
    /// constructs the value; this crate does not own the metadata schema yet.
    pub fn write_metadata(&self, metadata: &serde_json::Value) -> io::Result<()> {
        self.write_sidecar("metadata.json", metadata)
    }

    /// Write `result.json` (the final `{:status …}` envelope as JSON).
    pub fn write_result(&self, result: &serde_json::Value) -> io::Result<()> {
        self.write_sidecar("result.json", result)
    }

    /// Atomically-enough write a pretty-printed JSON sidecar into the run dir. These
    /// are whole-file writes (truncate), unlike the append-only events stream.
    fn write_sidecar(&self, name: &str, value: &serde_json::Value) -> io::Result<()> {
        let path = self.dir.join(name);
        let mut s = serde_json::to_string_pretty(value)
            .unwrap_or_else(|e| format!("{{\"error\":\"workflow {name} serialize: {e}\"}}"));
        s.push('\n');
        fs::write(path, s)
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

/// Pick the next free `events.resume-<n>.jsonl` segment filename under a run dir (1-based;
/// the first resume of a run writes `events.resume-1.jsonl`). A free-filename probe so
/// each resumed run gets its own clean segment (each keeps the frozen invariants).
pub fn next_resume_segment(runs_root: impl AsRef<Path>, run_id: &str) -> String {
    let dir = runs_root.as_ref().join(run_id);
    let mut n = 1;
    while dir.join(format!("events.resume-{n}.jsonl")).exists() {
        n += 1;
    }
    format!("events.resume-{n}.jsonl")
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
        let mut j = Journal::open(&root, "wf_test_0001").unwrap();
        j.write(&WorkflowEvent::RunStarted {
            seq: 0,
            ts: "0".into(),
            workflow: "hello-wf".into(),
            run_id: "wf_test_0001".into(),
            code_version: String::new(),
            args_json: String::new(),
        });
        j.write(&WorkflowEvent::RunEnded {
            seq: 1,
            ts: "0".into(),
            status: "success".into(),
            reason: None,
            dur_ms: 0,
        });
        drop(j); // flush/close

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
        j.write_args(&serde_json::json!({"name": "x"})).unwrap();
        j.write_result(&serde_json::json!({"status": "success"}))
            .unwrap();
        assert!(j.dir().join("args.json").exists());
        assert!(j.dir().join("result.json").exists());
        fs::remove_dir_all(&root).ok();
    }
}
