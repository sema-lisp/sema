//! Scoped, encrypted, file-backed [`TokenStore`] implementations for
//! workflow- and run-scoped MCP auth (`:persist :workflow` / `:persist :run`),
//! plus an in-memory store for `:persist :none`. See
//! `docs/plans/2026-06-24-workflow-mcp-auth.md` §4 — this module is the
//! persistence layer only; declaring `:mcp {alias {:persist …}}` and wiring it
//! into the workflow runtime is a later task.
//!
//! Unlike [`FileStore`](super::store::FileStore) — one shared document
//! readable in plaintext, relying solely on `0600` perms — a
//! [`ScopedFileStore`] writes one **encrypted** file per server under a
//! caller-supplied directory (a later task passes `.sema/auth/<workflow-name>/`
//! or `.sema/runs/<run-id>/auth/`), so a leaked directory (backup, misconfigured
//! `.gitignore`, `tar czf .`) is useless without the encryption key. The key
//! itself is resolved separately by [`store_encryption_key`] (env var or OS
//! keyring) and handed to [`ScopedFileStore::new`], so it never sits next to
//! the ciphertext it protects and tests can supply their own key without
//! touching the real keyring.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::{StoredCredentials, TokenStore};

/// Where a server's OAuth session is persisted, from a workflow's `:persist`
/// keyword (`:mcp {alias {:persist :workflow}}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistScope {
    /// OS keychain — shared across every workflow/run on this machine.
    Keyring,
    /// `.sema/auth/<workflow-name>/` — reused by every run of one workflow.
    Workflow,
    /// `.sema/runs/<run-id>/auth/` — this run only.
    Run,
    /// In-memory only; never touches disk.
    None,
}

impl std::str::FromStr for PersistScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "keyring" => Ok(Self::Keyring),
            "workflow" => Ok(Self::Workflow),
            "run" => Ok(Self::Run),
            "none" => Ok(Self::None),
            other => Err(format!(
                "invalid :persist value {other:?} (expected one of: keyring, workflow, run, none)"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// On-disk envelope
// ---------------------------------------------------------------------------

/// The on-disk JSON shape of one server's encrypted token file:
/// `{"v": 1, "nonce": "<base64>", "ciphertext": "<base64>"}`, where the
/// ciphertext is `serde_json(StoredCredentials)` sealed with ChaCha20-Poly1305.
#[derive(Serialize, Deserialize)]
struct Envelope {
    v: u8,
    nonce: String,
    ciphertext: String,
}

const ENVELOPE_VERSION: u8 = 1;

/// Derive a filesystem-safe, collision-resistant filename for a server URL:
/// the sanitized host (for a human-legible directory listing) plus a short
/// hash of the *full* URL (so two servers on the same host, e.g. differing
/// only by path, never collide).
fn server_filename(server_url: &str) -> String {
    let host = url::Url::parse(server_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "server".to_string());
    let sanitized: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let mut hasher = Sha256::new();
    hasher.update(server_url.as_bytes());
    let full_hash = format!("{:x}", hasher.finalize());
    let short_hash = &full_hash[..16];

    format!("{sanitized}-{short_hash}.json")
}

/// Encrypt `plaintext` under `key`, returning `(nonce_bytes, ciphertext)`.
/// A fresh random nonce is generated per call.
fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    let nonce =
        Nonce::try_generate().map_err(|e| format!("failed to generate a random nonce: {e}"))?;
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("encryption failed: {e}"))?;
    let nonce_bytes: [u8; 12] = nonce.into();
    Ok((nonce_bytes.to_vec(), ciphertext))
}

/// Decrypt a `(nonce_bytes, ciphertext)` pair under `key`. Fails (rather than
/// panicking) on a malformed nonce length, a wrong key, or tampered
/// ciphertext — all three surface identically as "can't decrypt this".
fn decrypt(key: &[u8; 32], nonce_bytes: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let nonce_arr: [u8; 12] = nonce_bytes
        .try_into()
        .map_err(|_| "invalid nonce length".to_string())?;
    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    cipher
        .decrypt(&Nonce::from(nonce_arr), ciphertext)
        .map_err(|e| format!("decryption failed: {e}"))
}

// ---------------------------------------------------------------------------
// Directory / file write helpers (mirrors FileStore::write_doc, but for a
// caller-chosen directory of many small files rather than one shared doc)
// ---------------------------------------------------------------------------

/// Create `dir` if missing and (re)assert `0700` permissions on it — self-
/// repairing, like `FileStore`'s perm handling, in case an earlier version or
/// a stray `mkdir` left it more permissive.
fn ensure_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create MCP auth dir {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("failed to set permissions on {}: {e}", dir.display()))?;
    }
    Ok(())
}

/// Write `bytes` to `path` atomically: a sibling temp file created `0600`
/// (never a world-readable window), then an atomic rename over the target —
/// which also fixes an existing wrongly-permissioned file. On Windows we rely
/// on the user profile ACL and just write in place.
#[cfg(unix)]
fn atomic_write_0600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .mode(0o600)
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|e| format!("failed to open temp token file: {e}"))?;
    file.write_all(bytes)
        .map_err(|e| format!("failed to write token file: {e}"))?;
    file.sync_all().ok();
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| format!("failed to replace token file: {e}"))
}

#[cfg(not(unix))]
fn atomic_write_0600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("failed to write token file: {e}"))
}

// ---------------------------------------------------------------------------
// Git-ignore guard (plan §4: a token file must never be committable)
// ---------------------------------------------------------------------------

/// A synthetic filename used only to ask git "would a file here be ignored?".
/// Probing a path *inside* `dir` rather than `dir` itself matters: git only
/// treats a trailing-slash (directory-only) `.gitignore` pattern like
/// `.sema/auth/` as matching a path it can confirm is a directory, and it
/// can't confirm that for a directory that doesn't exist yet — the exact
/// state of `dir` before the first save. A non-final path component doesn't
/// need that confirmation, so `dir/<probe>` matches correctly either way.
const GIT_IGNORE_PROBE_FILENAME: &str = ".sema-mcp-auth-probe.json";

/// Whether `git` is reachable and `dir` sits inside a git work tree that does
/// **not** ignore it. Missing `git` or "not a repository" both mean the guard
/// doesn't apply here (`Ok`); "in a repo, not ignored" is a hard `Err`.
///
/// Neither `dir` nor the probed file need exist yet — `git check-ignore`
/// matches purely on gitignore patterns — but the `git` subprocess itself
/// needs a real directory to run in, so we walk up to the nearest existing
/// ancestor.
fn git_ignore_ok(dir: &Path) -> Result<(), String> {
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(dir),
            Err(_) => return Ok(()),
        }
    };
    let probe_path = absolute.join(GIT_IGNORE_PROBE_FILENAME);

    let mut probe: &Path = &absolute;
    let run_dir = loop {
        if probe.exists() {
            break probe;
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return Ok(()),
        }
    };

    let output = match std::process::Command::new("git")
        .current_dir(run_dir)
        .arg("check-ignore")
        .arg("-q")
        .arg("--")
        .arg(&probe_path)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Ok(()), // git not on PATH
    };

    match output.status.code() {
        Some(0) => Ok(()),
        Some(1) => Err(format!(
            "refusing to write MCP token file under {} — it is inside a git repository but not \
             covered by .gitignore; add `.sema/` (or this specific auth directory) to \
             .gitignore before persisting tokens here",
            dir.display()
        )),
        // Typically 128 ("not a git repository"); nothing to enforce.
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// ScopedFileStore
// ---------------------------------------------------------------------------

/// One encrypted file per server under `dir`. Callers pass a workflow- or
/// run-scoped directory (`.sema/auth/<workflow-name>/`, `.sema/runs/<run-id>/auth/`)
/// and the key from [`store_encryption_key`]; this type never resolves the key
/// itself, so tests can pass an arbitrary one without touching the keyring.
pub struct ScopedFileStore {
    dir: PathBuf,
    key: [u8; 32],
    /// The git-ignore guard result, computed at most once per store instance
    /// (plan §4 asks for "once per store, not per save").
    git_guard: RefCell<Option<Result<(), String>>>,
}

impl ScopedFileStore {
    pub fn new(dir: PathBuf, key: [u8; 32]) -> Self {
        Self {
            dir,
            key,
            git_guard: RefCell::new(None),
        }
    }

    fn file_path(&self, server_url: &str) -> PathBuf {
        self.dir.join(server_filename(server_url))
    }

    fn ensure_git_ignored(&self) -> Result<(), String> {
        if let Some(cached) = self.git_guard.borrow().as_ref() {
            return cached.clone();
        }
        let result = git_ignore_ok(&self.dir);
        *self.git_guard.borrow_mut() = Some(result.clone());
        result
    }
}

impl TokenStore for ScopedFileStore {
    fn load(&self, server_url: &str) -> Option<StoredCredentials> {
        let path = self.file_path(server_url);
        let bytes = std::fs::read(&path).ok()?;

        let warn = |reason: &str| {
            eprintln!(
                "sema: MCP token file at {} {reason}; ignoring (re-authenticate)",
                path.display()
            );
        };

        let envelope: Envelope = match serde_json::from_slice(&bytes) {
            Ok(e) => e,
            Err(_) => {
                warn("is corrupt");
                return None;
            }
        };
        if envelope.v != ENVELOPE_VERSION {
            warn("has an unsupported format version");
            return None;
        }
        let engine = base64::engine::general_purpose::STANDARD;
        let Ok(nonce) = engine.decode(&envelope.nonce) else {
            warn("has an invalid nonce encoding");
            return None;
        };
        let Ok(ciphertext) = engine.decode(&envelope.ciphertext) else {
            warn("has an invalid ciphertext encoding");
            return None;
        };
        let Ok(plaintext) = decrypt(&self.key, &nonce, &ciphertext) else {
            warn("could not be decrypted (wrong key or corrupt data)");
            return None;
        };
        let creds: StoredCredentials = match serde_json::from_slice(&plaintext) {
            Ok(c) => c,
            Err(_) => {
                warn("decrypted to invalid data");
                return None;
            }
        };
        if creds.server_url != server_url {
            warn("is for a different server_url");
            return None;
        }
        Some(creds)
    }

    fn save(&self, creds: &StoredCredentials) -> Result<(), String> {
        self.ensure_git_ignored()?;
        ensure_dir(&self.dir)?;

        let plaintext =
            serde_json::to_vec(creds).map_err(|e| format!("failed to encode credentials: {e}"))?;
        let (nonce, ciphertext) = encrypt(&self.key, &plaintext)?;
        let engine = base64::engine::general_purpose::STANDARD;
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            nonce: engine.encode(nonce),
            ciphertext: engine.encode(ciphertext),
        };
        let json = serde_json::to_vec(&envelope)
            .map_err(|e| format!("failed to encode token envelope: {e}"))?;
        atomic_write_0600(&self.file_path(&creds.server_url), &json)
    }

    fn delete(&self, server_url: &str) -> Result<(), String> {
        let path = self.file_path(server_url);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!(
                "failed to delete MCP token file {}: {e}",
                path.display()
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryStore — `:persist :none`, never touches disk
// ---------------------------------------------------------------------------

/// An in-process, non-persistent [`TokenStore`]. Sema is single-threaded
/// (`Rc`, not `Arc`), so plain `RefCell` interior mutability is enough.
#[derive(Default)]
pub struct MemoryStore {
    entries: RefCell<HashMap<String, StoredCredentials>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for MemoryStore {
    fn load(&self, server_url: &str) -> Option<StoredCredentials> {
        self.entries
            .borrow()
            .get(server_url)
            .filter(|creds| creds.server_url == server_url)
            .cloned()
    }

    fn save(&self, creds: &StoredCredentials) -> Result<(), String> {
        self.entries
            .borrow_mut()
            .insert(creds.server_url.clone(), creds.clone());
        Ok(())
    }

    fn delete(&self, server_url: &str) -> Result<(), String> {
        self.entries.borrow_mut().remove(server_url);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Encryption-key resolution
// ---------------------------------------------------------------------------

const KEY_ENV_VAR: &str = "SEMA_MCP_AUTH_KEY";
const KEY_SERVICE: &str = "sema-mcp";
const KEY_ACCOUNT: &str = "store-encryption-key";

fn parse_hex_key(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!(
            "expected 64 hex characters (32 bytes), got {} character(s)",
            hex.len()
        ));
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        let digits = &hex[i * 2..i * 2 + 2];
        *byte = u8::from_str_radix(digits, 16)
            .map_err(|_| format!("invalid hex byte {digits:?} at position {i}"))?;
    }
    Ok(key)
}

fn encode_hex_key(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// Resolve the key used to encrypt/decrypt [`ScopedFileStore`] files:
///
/// 1. `SEMA_MCP_AUTH_KEY` env var (64 hex chars = 32 bytes) if set;
/// 2. else the OS keyring (service `"sema-mcp"`, account
///    `"store-encryption-key"`) — generating and saving a fresh random key on
///    first use;
/// 3. else an `Err` with an actionable hint naming `SEMA_MCP_AUTH_KEY`.
///
/// Deliberately separate from [`ScopedFileStore`] (which takes the key as a
/// plain parameter) so callers — and tests — can supply key material without
/// ever touching the real keyring.
pub fn store_encryption_key() -> Result<[u8; 32], String> {
    if let Ok(hex_key) = std::env::var(KEY_ENV_VAR) {
        return parse_hex_key(&hex_key).map_err(|e| {
            format!(
                "{KEY_ENV_VAR} is set but invalid ({e}); expected 64 hex characters (32 random \
                 bytes)"
            )
        });
    }

    let unavailable_hint = |e: &dyn std::fmt::Display| -> String {
        format!(
            "no {KEY_ENV_VAR} set and the OS keyring is unavailable ({e}); set {KEY_ENV_VAR} to \
             64 hex characters (32 random bytes) to use scoped file-backed token stores without \
             a keyring"
        )
    };

    let entry = keyring::Entry::new(KEY_SERVICE, KEY_ACCOUNT).map_err(|e| unavailable_hint(&e))?;

    match entry.get_password() {
        Ok(hex_key) => parse_hex_key(&hex_key).map_err(|e| {
            format!(
                "the encryption key stored in the OS keyring ({KEY_SERVICE}/{KEY_ACCOUNT}) is \
                 invalid ({e}); delete that keyring entry to regenerate it, or set \
                 {KEY_ENV_VAR} instead"
            )
        }),
        Err(keyring::Error::NoEntry) => {
            let key = <[u8; 32]>::try_generate()
                .map_err(|e| format!("failed to generate a random encryption key: {e}"))?;
            entry
                .set_password(&encode_hex_key(&key))
                .map_err(|e| unavailable_hint(&e))?;
            Ok(key)
        }
        Err(e) => Err(unavailable_hint(&e)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::{ClientInfo, StoredCredentials, TokenSet, TokenStore};
    use super::{store_encryption_key, Envelope, MemoryStore, PersistScope, ScopedFileStore};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const KEY_A: [u8; 32] = [7; 32];
    const KEY_B: [u8; 32] = [9; 32];

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("sema-mcp-scoped-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(url: &str) -> StoredCredentials {
        StoredCredentials {
            server_url: url.to_string(),
            tokens: TokenSet::from_response(
                "super-secret-access-token".into(),
                Some("refresh-1".into()),
                Some(3600),
                Some("files:read".into()),
                1_000,
            ),
            client_info: Some(ClientInfo {
                client_id: "client-1".into(),
                client_secret: None,
            }),
        }
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok()
    }

    #[test]
    fn scoped_file_store_round_trips() {
        let dir = temp_root().join("auth");
        let store = ScopedFileStore::new(dir, KEY_A);
        let creds = sample("https://mcp.example.com/mcp");
        store.save(&creds).unwrap();
        assert_eq!(store.load("https://mcp.example.com/mcp"), Some(creds));
        assert_eq!(store.load("https://other.example.com/mcp"), None);

        store.delete("https://mcp.example.com/mcp").unwrap();
        assert_eq!(store.load("https://mcp.example.com/mcp"), None);
        // Deleting a missing entry is a no-op, not an error.
        store.delete("https://mcp.example.com/mcp").unwrap();
    }

    #[test]
    fn scoped_file_store_ciphertext_hides_access_token() {
        let dir = temp_root().join("auth");
        let store = ScopedFileStore::new(dir.clone(), KEY_A);
        let creds = sample("https://mcp.example.com/mcp");
        store.save(&creds).unwrap();

        let mut hit_a_file = false;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains("super-secret-access-token"),
                "plaintext access token leaked into {}",
                path.display()
            );
            assert!(
                !text.contains("refresh-1"),
                "plaintext refresh token leaked"
            );
            assert!(text.contains("\"nonce\""));
            assert!(text.contains("\"ciphertext\""));
            hit_a_file = true;
        }
        assert!(hit_a_file, "expected save() to create a file");
    }

    #[test]
    fn scoped_file_store_uses_a_fresh_nonce_per_save() {
        let dir = temp_root().join("auth");
        let store = ScopedFileStore::new(dir.clone(), KEY_A);
        let creds = sample("https://mcp.example.com/mcp");

        let envelope_nonce = || {
            let mut path = None;
            for entry in std::fs::read_dir(&dir).unwrap() {
                path = Some(entry.unwrap().path());
            }
            let bytes = std::fs::read(path.unwrap()).unwrap();
            let envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
            envelope.nonce
        };

        store.save(&creds).unwrap();
        let first_nonce = envelope_nonce();

        store.save(&creds).unwrap();
        let second_nonce = envelope_nonce();

        assert_ne!(
            first_nonce, second_nonce,
            "two successive save()s of the same credentials must use independent random nonces"
        );
    }

    #[test]
    fn scoped_file_store_wrong_key_returns_none() {
        let dir = temp_root().join("auth");
        let writer = ScopedFileStore::new(dir.clone(), KEY_A);
        let creds = sample("https://mcp.example.com/mcp");
        writer.save(&creds).unwrap();

        let reader = ScopedFileStore::new(dir, KEY_B);
        assert_eq!(reader.load("https://mcp.example.com/mcp"), None);
    }

    #[test]
    fn scoped_file_store_corrupt_json_returns_none() {
        let dir = temp_root().join("auth");
        std::fs::create_dir_all(&dir).unwrap();
        let store = ScopedFileStore::new(dir.clone(), KEY_A);
        let path = dir.join("mcp.example.com-deadbeefcafebabe.json");
        std::fs::write(&path, b"{ not valid json").unwrap();
        assert_eq!(store.load("https://mcp.example.com/mcp"), None);
    }

    #[test]
    fn scoped_file_store_truncated_file_returns_none() {
        let dir = temp_root().join("auth");
        let store = ScopedFileStore::new(dir.clone(), KEY_A);
        let creds = sample("https://mcp.example.com/mcp");
        store.save(&creds).unwrap();

        let mut path = None;
        for entry in std::fs::read_dir(&dir).unwrap() {
            path = Some(entry.unwrap().path());
        }
        let path = path.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let truncated = &bytes[..bytes.len() / 2];
        std::fs::write(&path, truncated).unwrap();

        assert_eq!(store.load("https://mcp.example.com/mcp"), None);
    }

    #[cfg(unix)]
    #[test]
    fn scoped_file_store_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_root().join("auth");
        let store = ScopedFileStore::new(dir.clone(), KEY_A);
        store.save(&sample("https://mcp.example.com/mcp")).unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700, "auth dir must be owner-only");

        let mut count = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be owner-only");
            count += 1;
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn git_guard_blocks_non_ignored_dir_inside_repo() {
        if !git_available() {
            eprintln!("skipping git_guard_blocks_non_ignored_dir_inside_repo: git not on PATH");
            return;
        }
        let root = temp_root();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());

        let dir = root.join("auth-not-ignored");
        let store = ScopedFileStore::new(dir, KEY_A);
        let err = store
            .save(&sample("https://mcp.example.com/mcp"))
            .unwrap_err();
        assert!(
            err.contains("gitignore") || err.contains(".gitignore"),
            "error should mention .gitignore, got: {err}"
        );
    }

    #[test]
    fn git_guard_allows_ignored_dir_inside_repo() {
        if !git_available() {
            eprintln!("skipping git_guard_allows_ignored_dir_inside_repo: git not on PATH");
            return;
        }
        let root = temp_root();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(root.join(".gitignore"), "/auth-ignored/\n").unwrap();

        let dir = root.join("auth-ignored");
        let store = ScopedFileStore::new(dir, KEY_A);
        store.save(&sample("https://mcp.example.com/mcp")).unwrap();
    }

    #[test]
    fn git_guard_allows_outside_any_repo() {
        if !git_available() {
            eprintln!("skipping git_guard_allows_outside_any_repo: git not on PATH");
            return;
        }
        // temp_root() lives under the system temp dir, which is not inside a
        // git work tree in any supported CI/dev environment.
        let dir = temp_root().join("auth");
        let store = ScopedFileStore::new(dir, KEY_A);
        store.save(&sample("https://mcp.example.com/mcp")).unwrap();
    }

    #[test]
    fn memory_store_round_trips() {
        let store = MemoryStore::new();
        let creds = sample("https://mcp.example.com/mcp");
        store.save(&creds).unwrap();
        assert_eq!(store.load("https://mcp.example.com/mcp"), Some(creds));
        assert_eq!(store.load("https://other.example.com/mcp"), None);

        store.delete("https://mcp.example.com/mcp").unwrap();
        assert_eq!(store.load("https://mcp.example.com/mcp"), None);
        store.delete("https://mcp.example.com/mcp").unwrap();
    }

    #[test]
    fn persist_scope_parses_valid_values() {
        assert_eq!(
            "keyring".parse::<PersistScope>().unwrap(),
            PersistScope::Keyring
        );
        assert_eq!(
            "workflow".parse::<PersistScope>().unwrap(),
            PersistScope::Workflow
        );
        assert_eq!("run".parse::<PersistScope>().unwrap(), PersistScope::Run);
        assert_eq!("none".parse::<PersistScope>().unwrap(), PersistScope::None);
    }

    #[test]
    fn persist_scope_rejects_invalid_value() {
        let err = "bogus".parse::<PersistScope>().unwrap_err();
        assert!(err.contains("bogus"));
        assert!(err.contains("keyring"));
        assert!(err.contains("workflow"));
        assert!(err.contains("run"));
        assert!(err.contains("none"));
    }

    // SEMA_MCP_AUTH_KEY is a process-global env var; this is the only test
    // function that touches it so parallel test execution can't race on it.
    #[test]
    fn store_encryption_key_env_var_path() {
        std::env::remove_var("SEMA_MCP_AUTH_KEY");

        let hex = "11".repeat(32);
        std::env::set_var("SEMA_MCP_AUTH_KEY", &hex);
        let key = store_encryption_key().unwrap();
        assert_eq!(key, [0x11u8; 32]);

        std::env::set_var("SEMA_MCP_AUTH_KEY", "not-hex-and-wrong-length");
        let err = store_encryption_key().unwrap_err();
        assert!(err.contains("SEMA_MCP_AUTH_KEY"));

        std::env::remove_var("SEMA_MCP_AUTH_KEY");
    }
}
