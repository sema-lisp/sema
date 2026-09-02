//! Persistent MCP credential store, keyed by canonical server URL.
//!
//! Remote MCP auth is Sema's first persistent secret store. Two backends behind
//! one [`TokenStore`] trait: the OS keychain (primary) and a `0600` JSON file
//! (fallback for headless Linux/CI where no keychain is reachable). Stored per
//! server: the token set, the DCR client registration (so we don't re-register
//! every launch), and the `server_url` itself — validated on read so a stale
//! entry from a changed endpoint is ignored and re-auth runs.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Current wall-clock time in unix seconds (for token-expiry math).
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// An access/refresh token set. `expires_at` is absolute unix seconds, computed
/// from `expires_in` at receipt so expiry survives a restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scope: Option<String>,
}

impl TokenSet {
    /// Build from a token-endpoint response's fields at receipt time `now`.
    pub fn from_response(
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
        scope: Option<String>,
        now: u64,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            expires_at: expires_in.map(|secs| now.saturating_add(secs)),
            scope,
        }
    }

    /// Whether the access token is expired (or within `skew` seconds of it).
    /// A token with no expiry information is treated as non-expiring.
    pub fn is_expired(&self, now: u64, skew: u64) -> bool {
        match self.expires_at {
            Some(exp) => now.saturating_add(skew) >= exp,
            None => false,
        }
    }
}

/// A registered OAuth client (from Dynamic Client Registration or configured).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub client_secret: Option<String>,
}

/// Everything persisted for one MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub server_url: String,
    pub tokens: TokenSet,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub client_info: Option<ClientInfo>,
}

/// A credential backend. Implementations MUST validate that a loaded entry's
/// `server_url` matches the requested key.
pub trait TokenStore {
    fn load(&self, server_url: &str) -> Option<StoredCredentials>;
    fn save(&self, creds: &StoredCredentials) -> Result<(), String>;
    fn delete(&self, server_url: &str) -> Result<(), String>;

    /// Enumerate every stored credential set, sorted by server URL (for
    /// `sema mcp list`). Defaulted to a plain "unsupported" error because most
    /// backends have nothing to enumerate with — the OS keychain exposes no
    /// per-service listing API — and pretending such a store is empty would be
    /// worse than saying so.
    fn list(&self) -> Result<Vec<StoredCredentials>, String> {
        Err(
            "this credential store cannot enumerate servers (the OS keychain has no \
             listing API); only the file backend (SEMA_MCP_TOKEN_STORE=file) can be listed"
                .to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// File backend (0600 JSON), keyed by server URL
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileDoc {
    #[serde(default)]
    servers: std::collections::BTreeMap<String, StoredCredentials>,
}

pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Read the store. A missing file is an empty store; a file that exists but
    /// fails to parse is an ERROR (not silently discarded) so a corrupt or
    /// partially-written file never causes the next `save` to wipe every other
    /// server's credentials.
    fn read_doc(&self) -> Result<FileDoc, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                format!(
                    "MCP token store at {} is corrupt ({e}); refusing to overwrite it \
                     (fix or delete the file, then re-authenticate)",
                    self.path.display()
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileDoc::default()),
            Err(e) => Err(format!("failed to read token store: {e}")),
        }
    }

    fn write_doc(&self, doc: &FileDoc) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create token store dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(doc)
            .map_err(|e| format!("failed to encode token store: {e}"))?;

        // Write to a sibling temp file created 0600 (never a world-readable
        // window), then atomically rename over the target — which also fixes an
        // existing wrongly-permissioned file. On Windows we rely on the user
        // profile ACL and just write in place.
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let tmp = self.path.with_extension("json.tmp");
            let mut file = std::fs::OpenOptions::new()
                .mode(0o600)
                .create(true)
                .truncate(true)
                .write(true)
                .open(&tmp)
                .map_err(|e| format!("failed to open temp token store: {e}"))?;
            file.write_all(json.as_bytes())
                .map_err(|e| format!("failed to write token store: {e}"))?;
            file.sync_all().ok();
            drop(file);
            std::fs::rename(&tmp, &self.path)
                .map_err(|e| format!("failed to replace token store: {e}"))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&self.path, json)
                .map_err(|e| format!("failed to write token store: {e}"))?;
        }
        Ok(())
    }
}

impl TokenStore for FileStore {
    fn load(&self, server_url: &str) -> Option<StoredCredentials> {
        self.read_doc()
            .ok()?
            .servers
            .get(server_url)
            .filter(|creds| creds.server_url == server_url)
            .cloned()
    }

    fn save(&self, creds: &StoredCredentials) -> Result<(), String> {
        let mut doc = self.read_doc()?;
        doc.servers.insert(creds.server_url.clone(), creds.clone());
        self.write_doc(&doc)
    }

    fn delete(&self, server_url: &str) -> Result<(), String> {
        let mut doc = self.read_doc()?;
        doc.servers.remove(server_url);
        self.write_doc(&doc)
    }

    // `servers` is a BTreeMap keyed by URL, so the order is stable for free.
    fn list(&self) -> Result<Vec<StoredCredentials>, String> {
        Ok(self.read_doc()?.servers.into_values().collect())
    }
}

// ---------------------------------------------------------------------------
// Keychain backend
// ---------------------------------------------------------------------------

const KEYCHAIN_SERVICE: &str = "sema-mcp";

pub struct KeychainStore;

impl KeychainStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore for KeychainStore {
    fn load(&self, server_url: &str) -> Option<StoredCredentials> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, server_url).ok()?;
        let json = entry.get_password().ok()?;
        serde_json::from_str::<StoredCredentials>(&json)
            .ok()
            .filter(|creds| creds.server_url == server_url)
    }

    fn save(&self, creds: &StoredCredentials) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &creds.server_url)
            .map_err(|e| format!("keychain entry error: {e}"))?;
        let json = serde_json::to_string(creds)
            .map_err(|e| format!("failed to encode credentials: {e}"))?;
        entry
            .set_password(&json)
            .map_err(|e| format!("keychain write error: {e}"))
    }

    fn delete(&self, server_url: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, server_url)
            .map_err(|e| format!("keychain entry error: {e}"))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("keychain delete error: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Default store selection
// ---------------------------------------------------------------------------

/// The default on-disk path for the file fallback: `<config-dir>/sema/mcp-auth.json`
/// (honors `$XDG_CONFIG_HOME` on Linux).
pub fn default_file_path() -> PathBuf {
    if let Some(dirs) = directories::BaseDirs::new() {
        dirs.config_dir().join("sema").join("mcp-auth.json")
    } else {
        PathBuf::from(".sema-mcp-auth.json")
    }
}

/// Probe whether the OS keychain is usable in this environment by round-tripping
/// a throwaway entry.
fn keychain_available() -> bool {
    let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, "__sema_probe__") else {
        return false;
    };
    let ok = entry.set_password("probe").is_ok() && entry.get_password().is_ok();
    let _ = entry.delete_credential();
    ok
}

/// Pick the credential store: OS keychain when reachable, otherwise the `0600`
/// file fallback (with a one-time visible warning, since tokens then sit in a
/// permission-restricted plaintext file).
pub fn default_store() -> Box<dyn TokenStore> {
    // Explicit override via `SEMA_MCP_TOKEN_STORE=file|keychain`. `file` is the
    // handy escape hatch for dev/CI/headless: it skips the keychain entirely,
    // avoiding repeated macOS Keychain prompts — an ad-hoc-signed dev binary gets
    // a new code identity on every rebuild, so "Always Allow" never sticks.
    match std::env::var("SEMA_MCP_TOKEN_STORE").ok().as_deref() {
        Some("file") => return Box::new(FileStore::new(default_file_path())),
        Some("keychain") => return Box::new(KeychainStore::new()),
        _ => {}
    }
    if keychain_available() {
        Box::new(KeychainStore::new())
    } else {
        let path = default_file_path();
        eprintln!(
            "sema: OS keychain unavailable; storing MCP tokens in {} (file perms 0600, plaintext).",
            path.display()
        );
        Box::new(FileStore::new(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        sema_core::testing::unique_temp_dir("mcp-store").join("mcp-auth.json")
    }

    fn sample(url: &str) -> StoredCredentials {
        StoredCredentials {
            server_url: url.to_string(),
            tokens: TokenSet::from_response(
                "access-1".into(),
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

    #[test]
    fn file_store_round_trips() {
        let store = FileStore::new(temp_path());
        let creds = sample("https://mcp.example.com/mcp");
        store.save(&creds).unwrap();
        assert_eq!(store.load("https://mcp.example.com/mcp"), Some(creds));
        assert_eq!(store.load("https://other.example.com/mcp"), None);
    }

    #[test]
    fn corrupt_store_is_not_clobbered_on_save() {
        let store = FileStore::new(temp_path());
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), "{ not valid json").unwrap();
        // save must refuse (returning Err) rather than wipe the file…
        assert!(store.save(&sample("https://x.example.com/mcp")).is_err());
        // …and the file must be untouched.
        assert_eq!(
            std::fs::read_to_string(store.path()).unwrap(),
            "{ not valid json"
        );
        // load of a corrupt store yields None (→ re-auth), never a panic.
        assert!(store.load("https://x.example.com/mcp").is_none());
    }

    #[test]
    fn file_store_delete_removes_entry() {
        let store = FileStore::new(temp_path());
        let creds = sample("https://mcp.example.com/mcp");
        store.save(&creds).unwrap();
        store.delete("https://mcp.example.com/mcp").unwrap();
        assert_eq!(store.load("https://mcp.example.com/mcp"), None);
        // Deleting a missing entry is a no-op, not an error.
        store.delete("https://mcp.example.com/mcp").unwrap();
    }

    #[test]
    fn file_store_keeps_multiple_servers_isolated() {
        let store = FileStore::new(temp_path());
        let a = sample("https://a.example.com/mcp");
        let b = sample("https://b.example.com/mcp");
        store.save(&a).unwrap();
        store.save(&b).unwrap();
        assert_eq!(store.load("https://a.example.com/mcp"), Some(a));
        assert_eq!(store.load("https://b.example.com/mcp"), Some(b));
    }

    #[test]
    fn file_store_lists_servers_in_url_order() {
        let store = FileStore::new(temp_path());
        assert_eq!(store.list().unwrap(), vec![]);
        store.save(&sample("https://b.example.com/mcp")).unwrap();
        store.save(&sample("https://a.example.com/mcp")).unwrap();
        let urls: Vec<String> = store
            .list()
            .unwrap()
            .into_iter()
            .map(|c| c.server_url)
            .collect();
        assert_eq!(
            urls,
            ["https://a.example.com/mcp", "https://b.example.com/mcp"]
        );
    }

    #[test]
    fn keychain_list_is_unsupported_not_empty() {
        // No keychain access needed: the default trait impl answers before any
        // backend call, and the message must steer at the file backend.
        let err = KeychainStore::new().list().unwrap_err();
        assert!(err.contains("SEMA_MCP_TOKEN_STORE=file"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let store = FileStore::new(temp_path());
        store.save(&sample("https://mcp.example.com/mcp")).unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "token file must be owner-only");
    }

    #[test]
    fn token_expiry_math() {
        let mut t = TokenSet::from_response("a".into(), None, Some(3600), None, 1_000);
        assert_eq!(t.expires_at, Some(4_600));
        assert!(!t.is_expired(4_000, 60));
        assert!(t.is_expired(4_600, 0));
        // within the skew window counts as expired
        assert!(t.is_expired(4_570, 60));
        // no expiry info => never expired
        t.expires_at = None;
        assert!(!t.is_expired(u64::MAX, 0));
    }
}
