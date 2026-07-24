// Fixture: the SANCTIONED A2+A3 workflow-journal filesystem shape. `create_dir_all` is
// used only for the parent chain; the run dir is created non-recursively and the journal
// file is claimed atomically with `create_new`. Segment claims use `create_new` in a loop
// — no exists-probe. Since A3 journal.rs is WRITE-FREE: every event/memo/sidecar write
// (and the memo subdir's create_dir_all) lives on the writer THREAD, so there is no
// `write_all` or `fs::write` here — the VM-thread journal methods only enqueue. This
// fixture must PASS both the journal (A2) and writer (A3) policies.
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;

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

// A memo is ENQUEUED to the writer thread — no filesystem write on the VM thread.
fn write_memo(writer: &JournalWriter, key: &str, json: String) {
    writer.enqueue_memo(key.to_string(), json);
}

fn claim_segment(dir: &Path) -> io::Result<()> {
    let mut n = 1u32;
    loop {
        let path = dir.join(format!("events.resume-{n}.jsonl"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_file) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    }
}
