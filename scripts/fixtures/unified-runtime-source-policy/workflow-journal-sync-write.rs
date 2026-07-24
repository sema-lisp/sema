// Fixture: a REGRESSION of the A3 writer policy. journal.rs must stay WRITE-FREE — every
// event/memo/sidecar write lives on the writer thread. Here `Journal::write` reintroduces a
// SYNCHRONOUS `write_all` on the VM thread (and `write_sidecar` a synchronous `fs::write`),
// exactly the blocking-I/O-on-the-quantum shape A3 removed. This fixture must FAIL the
// writer policy against the zero allowlist (WORKFLOW_WRITE_ALL / WORKFLOW_FS_WRITE have no
// row).
use std::fs;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

struct Journal {
    dir: std::path::PathBuf,
    writer: BufWriter<File>,
}

impl Journal {
    // FORBIDDEN: a synchronous journal write back on the VM thread.
    fn write(&mut self, line: &str) {
        let _ = self.writer.write_all(line.as_bytes());
        let _ = self.writer.flush();
    }

    // FORBIDDEN: a synchronous sidecar write back on the VM thread.
    fn write_sidecar(&self, name: &Path, json: &str) {
        let _ = fs::write(self.dir.join(name), json);
    }
}
