//! Small helpers for tests across the workspace.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty directory under the system temp dir, unique per process
/// and per call, so parallel tests never share state. The caller removes it.
pub fn unique_temp_dir(tag: &str) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("sema-{tag}-{}-{n}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

/// A temp directory that is deleted when dropped, including on panic.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        Self(unique_temp_dir(tag))
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
