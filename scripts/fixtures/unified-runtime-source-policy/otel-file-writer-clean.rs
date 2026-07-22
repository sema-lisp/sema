// Fixture: the sanctioned C2 shape for a non-writer sema-otel path. `export()` renders each
// span to a String on the VM thread and only `try_send`s it to the writer thread — it
// performs NO `write_all`/`fs::write` itself. This file must PASS the sema-otel file-writer
// policy against the zero allowlist (the writes live on the writer thread, not here).
use std::sync::mpsc::SyncSender;

enum WriterMsg {
    Line(String),
}

struct JsonlFileExporter {
    tx: SyncSender<WriterMsg>,
}

impl JsonlFileExporter {
    // OK: render on the VM thread, enqueue to the writer thread — no filesystem write here.
    fn enqueue(&self, line: String) {
        let _ = self.tx.try_send(WriterMsg::Line(line));
    }
}
