//! Virtual filesystem shim for notebook file access.
//!
//! All paths are scoped to the notebook's directory to prevent
//! unauthorized filesystem access. Path traversal attempts are rejected.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// A directory entry returned by the list endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Resolve a relative path within the VFS root, rejecting traversal.
fn resolve_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim_start_matches('/');
    let boundary = sema_core::path::PathBoundary::new(root)
        .map_err(|error| format!("VFS root error: {error}"))?;
    boundary
        .resolve(&root.join(relative))
        .map_err(|error| format!("Path traversal denied: {error}"))
}

/// Read a file within the VFS root.
pub fn read_file(root: &Path, relative: &str) -> Result<String, String> {
    let path = resolve_path(root, relative)?;
    std::fs::read_to_string(&path).map_err(|e| format!("Read error: {e}"))
}

/// Write a file within the VFS root.
pub fn write_file(root: &Path, relative: &str, content: &str) -> Result<(), String> {
    let path = resolve_path(root, relative)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Mkdir error: {e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("Write error: {e}"))
}

/// List directory contents within the VFS root.
pub fn list_dir(root: &Path, relative: &str) -> Result<Vec<FileEntry>, String> {
    let path = resolve_path(root, relative)?;
    if !path.is_dir() {
        return Err(format!("Not a directory: {}", relative));
    }

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&path).map_err(|e| format!("List error: {e}"))?;

    for entry in read_dir {
        let entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let meta = entry.metadata().ok();
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
            size: meta
                .as_ref()
                .and_then(|m| if m.is_file() { Some(m.len()) } else { None }),
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}
