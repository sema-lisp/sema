// Fixture: a REGRESSION of the A2 journal policy. The `create_dir_all` count still matches
// the allowlist (parent chain only), but `next_resume_segment` reintroduces the exists-probe
// segment claim (`.exists()` in a loop) that A2 replaced with an atomic `create_new`.
// That TOCTOU probe lets two concurrent resumes double-claim a segment, so this fixture
// must FAIL the policy (WORKFLOW_SEGMENT_EXISTS_PROBE has no allowlist row).
use std::fs;
use std::io;
use std::path::Path;

fn ensure_run_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(dir)
}

// FORBIDDEN: an exists-probe segment claim (the pre-A2 TOCTOU shape).
fn next_resume_segment(dir: &Path) -> String {
    let mut n = 1;
    while dir.join(format!("events.resume-{n}.jsonl")).exists() {
        n += 1;
    }
    format!("events.resume-{n}.jsonl")
}
