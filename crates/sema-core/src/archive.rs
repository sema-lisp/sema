//! VFS archive binary format for bundling files into a standalone executable.
//!
//! The archive is appended to the sema binary to create a self-contained
//! executable. It stores metadata key-value pairs and file contents, with
//! a CRC32 checksum for integrity validation.
//!
//! ## Binary Layout
//!
//! ```text
//! Header:
//!   format_version: u16 LE (= 1)
//!   flags:          u16 LE (= 0, reserved)
//!   checksum:       u32 LE (CRC32 of everything after this field)
//!   metadata_count: u32 LE
//! Metadata entries (repeated metadata_count times):
//!   key_len: u16 LE
//!   key:     [u8; key_len] (UTF-8)
//!   val_len: u32 LE
//!   val:     [u8; val_len]
//! TOC:
//!   entry_count: u32 LE
//!   entries (repeated entry_count times):
//!     path_len: u32 LE
//!     path:     [u8; path_len] (UTF-8)
//!     offset:   u64 LE (relative to file data start)
//!     size:     u64 LE
//! File data:
//!   raw bytes for all files
//!
//! Trailer (appended after archive, for ELF detection):
//!   archive_size: u64 LE
//!   magic:        "SEMAEXEC" (8 bytes)
//! ```

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Magic bytes written at the end of a bundled executable.
pub const MAGIC: &[u8; 8] = b"SEMAEXEC";

/// Size of the trailer in bytes (u64 archive_size + 8-byte magic).
pub const TRAILER_SIZE: usize = 16;

/// Current archive format version.
pub const FORMAT_VERSION: u16 = 1;

// ---------------------------------------------------------------------------
// Archive struct
// ---------------------------------------------------------------------------

/// An in-memory representation of a VFS archive.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Archive {
    pub format_version: u16,
    pub flags: u16,
    pub metadata: HashMap<String, Vec<u8>>,
    pub files: HashMap<String, Vec<u8>>,
}

impl Default for Archive {
    fn default() -> Self {
        Self::new()
    }
}

impl Archive {
    /// Create a new empty archive with the current format version.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            flags: 0,
            metadata: HashMap::new(),
            files: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Check whether `path` contains an embedded SEMAEXEC archive by reading the
/// last 16 bytes and checking for the magic trailer.
pub fn has_embedded_archive(path: &Path) -> io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len < TRAILER_SIZE as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-(TRAILER_SIZE as i64)))?;
    let mut trailer = [0u8; TRAILER_SIZE];
    file.read_exact(&mut trailer)?;
    let magic = &trailer[8..16];
    Ok(magic == MAGIC)
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Read a bundled executable at `path`, locate the embedded archive using the
/// trailer, and deserialize it.
#[allow(dead_code)]
pub fn extract_archive(path: &Path) -> io::Result<Archive> {
    let data = std::fs::read(path)?;
    let len = data.len();

    if len < TRAILER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file too small to contain archive trailer",
        ));
    }

    // Read trailer
    let trailer = &data[len - TRAILER_SIZE..];
    let magic = &trailer[8..16];
    if magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SEMAEXEC magic not found in trailer",
        ));
    }

    let archive_size = u64::from_le_bytes(trailer[0..8].try_into().unwrap()) as usize;
    let archive_start = len - TRAILER_SIZE - archive_size;

    if archive_start > len - TRAILER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive size exceeds file size",
        ));
    }

    let archive_bytes = &data[archive_start..archive_start + archive_size];
    deserialize_archive(archive_bytes)
}

// ---------------------------------------------------------------------------
// Deserialization (private)
// ---------------------------------------------------------------------------

/// Helper to read a `u16` LE from a cursor position, advancing it.
fn read_u16(data: &[u8], pos: &mut usize) -> io::Result<u16> {
    if *pos + 2 > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unexpected end of archive (u16)",
        ));
    }
    let val = u16::from_le_bytes(data[*pos..*pos + 2].try_into().unwrap());
    *pos += 2;
    Ok(val)
}

/// Helper to read a `u32` LE from a cursor position, advancing it.
fn read_u32(data: &[u8], pos: &mut usize) -> io::Result<u32> {
    if *pos + 4 > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unexpected end of archive (u32)",
        ));
    }
    let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(val)
}

/// Helper to read a `u64` LE from a cursor position, advancing it.
fn read_u64(data: &[u8], pos: &mut usize) -> io::Result<u64> {
    if *pos + 8 > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unexpected end of archive (u64)",
        ));
    }
    let val = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

/// Helper to read `n` bytes from a cursor position, advancing it.
fn read_bytes<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> io::Result<&'a [u8]> {
    if *pos + n > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unexpected end of archive (bytes)",
        ));
    }
    let slice = &data[*pos..*pos + n];
    *pos += n;
    Ok(slice)
}

/// Parse raw archive bytes into an `Archive`. Validates the CRC32 checksum.
fn deserialize_archive(data: &[u8]) -> io::Result<Archive> {
    let mut pos = 0;

    // Header
    let format_version = read_u16(data, &mut pos)?;
    if format_version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported archive format version {format_version}, expected {FORMAT_VERSION}"
            ),
        ));
    }

    let flags = read_u16(data, &mut pos)?;
    let stored_checksum = read_u32(data, &mut pos)?;
    // pos is now 8 -- everything from here on is checksummed
    let checksum_start = pos;

    // Validate CRC32
    let computed_checksum = crc32fast::hash(&data[checksum_start..]);
    if stored_checksum != computed_checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "archive checksum mismatch: stored {stored_checksum:#010x}, computed {computed_checksum:#010x}"
            ),
        ));
    }

    // Metadata
    let metadata_count = read_u32(data, &mut pos)? as usize;
    // Clamp capacity to avoid OOM from malicious archives — each metadata entry
    // is at least 8 bytes (u16 key_len + u32 val_len + 2 bytes min), so the
    // remaining data bounds how many entries can actually exist.
    let remaining = data.len().saturating_sub(pos);
    let mut metadata = HashMap::with_capacity(metadata_count.min(remaining / 8));
    for _ in 0..metadata_count {
        let key_len = read_u16(data, &mut pos)? as usize;
        let key_bytes = read_bytes(data, &mut pos, key_len)?;
        let key = String::from_utf8(key_bytes.to_vec()).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("metadata key is not valid UTF-8: {e}"),
            )
        })?;
        let val_len = read_u32(data, &mut pos)? as usize;
        let val = read_bytes(data, &mut pos, val_len)?.to_vec();
        metadata.insert(key, val);
    }

    // TOC
    let entry_count = read_u32(data, &mut pos)? as usize;

    struct TocEntry {
        path: String,
        offset: u64,
        size: u64,
    }

    // Clamp capacity — each TOC entry is at least 20 bytes (u32 path_len + u64 offset + u64 size).
    let remaining = data.len().saturating_sub(pos);
    let mut toc = Vec::with_capacity(entry_count.min(remaining / 20));
    for _ in 0..entry_count {
        let path_len = read_u32(data, &mut pos)? as usize;
        let path_bytes = read_bytes(data, &mut pos, path_len)?;
        let path = String::from_utf8(path_bytes.to_vec()).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("file path is not valid UTF-8: {e}"),
            )
        })?;
        let offset = read_u64(data, &mut pos)?;
        let size = read_u64(data, &mut pos)?;
        toc.push(TocEntry { path, offset, size });
    }

    // File data starts at current pos
    let file_data_start = pos;
    let mut files = HashMap::with_capacity(toc.len());
    for entry in &toc {
        let start = file_data_start + entry.offset as usize;
        let end = start + entry.size as usize;
        if end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "file entry '{}' extends beyond archive (offset={}, size={}, data_len={})",
                    entry.path,
                    entry.offset,
                    entry.size,
                    data.len()
                ),
            ));
        }
        files.insert(entry.path.clone(), data[start..end].to_vec());
    }

    Ok(Archive {
        format_version,
        flags,
        metadata,
        files,
    })
}

/// Public entry point for deserializing archive bytes (used by the libsui
/// path where we already have raw bytes extracted from the binary).
pub fn deserialize_archive_from_bytes(data: &[u8]) -> io::Result<Archive> {
    deserialize_archive(data)
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Build archive bytes from metadata and file maps.
///
/// Keys are sorted before serialization for deterministic output. The CRC32
/// checksum is computed over everything after the checksum field and then
/// backfilled into position.
pub fn serialize_archive(
    metadata: &HashMap<String, Vec<u8>>,
    files: &HashMap<String, Vec<u8>>,
) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();

    // -- Header --
    buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes()); // format_version: u16
    buf.extend_from_slice(&0u16.to_le_bytes()); // flags: u16
    buf.extend_from_slice(&0u32.to_le_bytes()); // checksum placeholder: u32
                                                // checksum field ends at byte 8

    // -- Metadata --
    let mut meta_keys: Vec<&String> = metadata.keys().collect();
    meta_keys.sort();

    buf.extend_from_slice(&(meta_keys.len() as u32).to_le_bytes()); // metadata_count
    for key in &meta_keys {
        let key_bytes = key.as_bytes();
        buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(key_bytes);
        let val = &metadata[*key];
        buf.extend_from_slice(&(val.len() as u32).to_le_bytes());
        buf.extend_from_slice(val);
    }

    // -- TOC + File data --
    // We need to build the TOC and file data together so offsets are correct.
    let mut file_keys: Vec<&String> = files.keys().collect();
    file_keys.sort();

    // First pass: compute offsets
    struct FileEntry<'a> {
        path: &'a str,
        data: &'a [u8],
        offset: u64,
    }

    let mut entries = Vec::with_capacity(file_keys.len());
    let mut current_offset: u64 = 0;
    for key in &file_keys {
        let data = &files[*key];
        entries.push(FileEntry {
            path: key.as_str(),
            data,
            offset: current_offset,
        });
        current_offset += data.len() as u64;
    }

    // Write entry_count
    buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());

    // Write TOC entries
    for entry in &entries {
        let path_bytes = entry.path.as_bytes();
        buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(path_bytes);
        buf.extend_from_slice(&entry.offset.to_le_bytes());
        buf.extend_from_slice(&(entry.data.len() as u64).to_le_bytes());
    }

    // Write file data
    for entry in &entries {
        buf.extend_from_slice(entry.data);
    }

    // -- Backfill CRC32 --
    let checksum = crc32fast::hash(&buf[8..]); // everything after the checksum field
    buf[4..8].copy_from_slice(&checksum.to_le_bytes());

    buf
}

// ---------------------------------------------------------------------------
// Bundle writer
// ---------------------------------------------------------------------------

/// Copy the runtime binary at `runtime_path` to `output_path`, then append the
/// archive bytes and a 16-byte trailer. On Unix, the output is made executable.
#[allow(dead_code)]
pub fn write_bundled_executable(
    runtime_path: &Path,
    output_path: &Path,
    archive_bytes: &[u8],
) -> io::Result<()> {
    // Read the runtime binary
    let runtime = std::fs::read(runtime_path)?;

    // Write: runtime + archive + trailer
    let mut out = std::fs::File::create(output_path)?;
    out.write_all(&runtime)?;
    out.write_all(archive_bytes)?;

    // Trailer
    let archive_size = archive_bytes.len() as u64;
    out.write_all(&archive_size.to_le_bytes())?;
    out.write_all(MAGIC)?;

    out.flush()?;
    drop(out);

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(output_path, perms)?;
    }

    Ok(())
}

/// Like `write_bundled_executable`, but takes the runtime binary as a byte
/// slice instead of reading from disk. Used for cross-compilation where the
/// runtime bytes were downloaded/cached.
pub fn write_bundled_executable_from_bytes(
    runtime: &[u8],
    output_path: &Path,
    archive_bytes: &[u8],
) -> io::Result<()> {
    let mut out = std::fs::File::create(output_path)?;
    out.write_all(runtime)?;
    out.write_all(archive_bytes)?;

    // Trailer
    let archive_size = archive_bytes.len() as u64;
    out.write_all(&archive_size.to_le_bytes())?;
    out.write_all(MAGIC)?;

    out.flush()?;
    drop(out);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(output_path, perms)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_roundtrip() {
        let mut metadata = HashMap::new();
        metadata.insert("entry".to_string(), b"main.semac".to_vec());
        metadata.insert("version".to_string(), b"1".to_vec());

        let mut files = HashMap::new();
        files.insert("main.semac".to_string(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
        files.insert("lib/utils.sema".to_string(), b"(define x 42)".to_vec());

        let bytes = serialize_archive(&metadata, &files);
        let archive = deserialize_archive(&bytes).expect("deserialize should succeed");

        assert_eq!(archive.format_version, FORMAT_VERSION);
        assert_eq!(archive.flags, 0);
        assert_eq!(archive.metadata.len(), 2);
        assert_eq!(archive.metadata.get("entry").unwrap(), b"main.semac");
        assert_eq!(archive.metadata.get("version").unwrap(), b"1");
        assert_eq!(archive.files.len(), 2);
        assert_eq!(
            archive.files.get("main.semac").unwrap(),
            &vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
        assert_eq!(
            archive.files.get("lib/utils.sema").unwrap(),
            b"(define x 42)"
        );
    }

    #[test]
    fn test_archive_empty() {
        let metadata = HashMap::new();
        let files = HashMap::new();

        let bytes = serialize_archive(&metadata, &files);
        let archive = deserialize_archive(&bytes).expect("deserialize should succeed");

        assert_eq!(archive.format_version, FORMAT_VERSION);
        assert_eq!(archive.flags, 0);
        assert!(archive.metadata.is_empty());
        assert!(archive.files.is_empty());
    }

    #[test]
    fn test_crc32_known_value() {
        // CRC32 of empty data should be 0x00000000
        // Actually, CRC32 of empty is 0x00000000 for our implementation
        let empty = crc32fast::hash(b"");
        assert_eq!(empty, 0x0000_0000);

        // CRC32 of "123456789" is 0xCBF43926 (well-known test vector)
        let check = crc32fast::hash(b"123456789");
        assert_eq!(check, 0xCBF4_3926);
    }

    #[test]
    fn test_checksum_validation() {
        let metadata = HashMap::new();
        let files = HashMap::new();
        let mut bytes = serialize_archive(&metadata, &files);

        // Corrupt the checksum
        bytes[4] ^= 0xFF;

        let result = deserialize_archive(&bytes);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("checksum mismatch"),
            "error should mention checksum: {err}"
        );
    }

    #[test]
    fn test_deserialize_archive_from_bytes_public() {
        let metadata = HashMap::new();
        let mut files = HashMap::new();
        files.insert("test.txt".to_string(), b"hello".to_vec());

        let bytes = serialize_archive(&metadata, &files);
        let archive =
            deserialize_archive_from_bytes(&bytes).expect("public deserialize should succeed");

        assert_eq!(archive.files.len(), 1);
        assert_eq!(archive.files.get("test.txt").unwrap(), b"hello");
    }

    /// Build a minimal archive with a tampered metadata_count or entry_count.
    /// Recomputes the CRC32 so the checksum passes.
    fn craft_archive_with_counts(metadata_count: u32, entry_count: u32) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        // Header
        buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // flags
        buf.extend_from_slice(&0u32.to_le_bytes()); // checksum placeholder

        // metadata_count (potentially huge)
        buf.extend_from_slice(&metadata_count.to_le_bytes());
        // No actual metadata entries — the loop will hit EOF immediately

        // entry_count (potentially huge)
        buf.extend_from_slice(&entry_count.to_le_bytes());
        // No actual TOC entries

        // Backfill CRC32
        let checksum = crc32fast::hash(&buf[8..]);
        buf[4..8].copy_from_slice(&checksum.to_le_bytes());
        buf
    }

    #[test]
    fn test_huge_metadata_count_does_not_oom() {
        // A crafted archive claiming u32::MAX metadata entries but containing none.
        // Should fail gracefully with an error, not panic/OOM.
        let data = craft_archive_with_counts(u32::MAX, 0);
        let result = deserialize_archive(&data);
        assert!(result.is_err(), "should fail, not OOM");
    }

    #[test]
    fn test_huge_entry_count_does_not_oom() {
        // A crafted archive claiming u32::MAX file entries but containing none.
        let data = craft_archive_with_counts(0, u32::MAX);
        let result = deserialize_archive(&data);
        assert!(result.is_err(), "should fail, not OOM");
    }

    #[test]
    fn test_write_and_detect_bundled() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("sema_archive_test");
        let _ = std::fs::create_dir_all(&dir);

        let runtime_path = dir.join("fake_runtime");
        let output_path = dir.join("bundled_output");

        // Create a fake "runtime" binary
        {
            let mut f = std::fs::File::create(&runtime_path).unwrap();
            f.write_all(b"FAKE_RUNTIME_BINARY").unwrap();
        }

        // Build archive
        let mut metadata = HashMap::new();
        metadata.insert("entry".to_string(), b"main.semac".to_vec());
        let mut files = HashMap::new();
        files.insert("main.semac".to_string(), vec![1, 2, 3, 4]);

        let archive_bytes = serialize_archive(&metadata, &files);

        // Write bundled executable
        write_bundled_executable(&runtime_path, &output_path, &archive_bytes).unwrap();

        // Should detect the archive
        assert!(has_embedded_archive(&output_path).unwrap());

        // Should NOT detect archive in the plain runtime
        assert!(!has_embedded_archive(&runtime_path).unwrap());

        // Extract and verify
        let extracted = extract_archive(&output_path).unwrap();
        assert_eq!(extracted.metadata.get("entry").unwrap(), b"main.semac");
        assert_eq!(
            extracted.files.get("main.semac").unwrap(),
            &vec![1, 2, 3, 4]
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_bundled_from_bytes_roundtrip() {
        let dir = std::env::temp_dir().join("sema_archive_from_bytes_test");
        let _ = std::fs::create_dir_all(&dir);
        let output_path = dir.join("bundled_from_bytes");

        let runtime = b"FAKE_RUNTIME_BINARY";
        let mut metadata = HashMap::new();
        metadata.insert("entry".to_string(), b"main.semac".to_vec());
        let mut files = HashMap::new();
        files.insert("main.semac".to_string(), vec![1, 2, 3, 4]);
        let archive_bytes = serialize_archive(&metadata, &files);

        write_bundled_executable_from_bytes(runtime, &output_path, &archive_bytes).unwrap();

        assert!(has_embedded_archive(&output_path).unwrap());
        let extracted = extract_archive(&output_path).unwrap();
        assert_eq!(extracted.metadata.get("entry").unwrap(), b"main.semac");
        assert_eq!(
            extracted.files.get("main.semac").unwrap(),
            &vec![1, 2, 3, 4]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
