// Fixture: the SANCTIONED A2 workflow-journal filesystem shape. `create_dir_all` is used
// only for the parent chain and the memo subdir; the run dir is created non-recursively
// and the journal file is claimed atomically with `create_new`. Segment claims use
// `create_new` in a loop — no exists-probe. This fixture must PASS the policy.
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

fn write_memo(dir: &Path, key: &str, json: &str) -> io::Result<()> {
    let memo_dir = dir.join("memo");
    fs::create_dir_all(&memo_dir)?;
    fs::write(memo_dir.join(format!("{key}.json")), json)
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
