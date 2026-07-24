// Fixture: a REGRESSION of the C2 writer policy. A non-writer sema-otel path must stay
// WRITE-FREE — every span line is written on the file_exporter.rs writer thread. Here
// `export()` reintroduces a SYNCHRONOUS `write_all` on the VM thread (and a synchronous
// `fs::write` sidecar), exactly the per-span-end blocking-I/O shape C2 removed. This fixture
// must FAIL the writer policy against the zero allowlist (SEMA_OTEL_WRITE_ALL /
// SEMA_OTEL_FS_WRITE have no row).
use std::fs;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

struct JsonlFileExporter {
    dir: std::path::PathBuf,
    writer: BufWriter<File>,
}

impl JsonlFileExporter {
    // FORBIDDEN: a synchronous per-span write back on the VM thread.
    fn export(&mut self, line: &str) {
        let _ = self.writer.write_all(line.as_bytes());
        let _ = self.writer.flush();
    }

    // FORBIDDEN: a synchronous sidecar write back on the VM thread.
    fn dump(&self, name: &Path, json: &str) {
        let _ = fs::write(self.dir.join(name), json);
    }
}
