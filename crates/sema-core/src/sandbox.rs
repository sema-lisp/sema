use std::fmt;
use std::path::PathBuf;

use crate::error::SemaError;
use crate::path::PathExt as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps(u64);

impl Caps {
    pub const NONE: Caps = Caps(0);
    pub const FS_READ: Caps = Caps(1 << 0);
    pub const FS_WRITE: Caps = Caps(1 << 1);
    pub const SHELL: Caps = Caps(1 << 2);
    pub const NETWORK: Caps = Caps(1 << 3);
    pub const ENV_READ: Caps = Caps(1 << 4);
    pub const ENV_WRITE: Caps = Caps(1 << 5);
    pub const PROCESS: Caps = Caps(1 << 6);
    pub const LLM: Caps = Caps(1 << 7);
    pub const SERIAL: Caps = Caps(1 << 8);

    pub const ALL: Caps = Caps(
        Self::FS_READ.0
            | Self::FS_WRITE.0
            | Self::SHELL.0
            | Self::NETWORK.0
            | Self::ENV_READ.0
            | Self::ENV_WRITE.0
            | Self::PROCESS.0
            | Self::LLM.0
            | Self::SERIAL.0,
    );

    pub const STRICT: Caps = Caps(
        Self::SHELL.0
            | Self::FS_WRITE.0
            | Self::NETWORK.0
            | Self::ENV_WRITE.0
            | Self::PROCESS.0
            | Self::LLM.0
            | Self::SERIAL.0,
    );

    pub fn contains(self, other: Caps) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Caps) -> Caps {
        Caps(self.0 | other.0)
    }

    pub fn name(self) -> &'static str {
        match self {
            Caps::NONE => "none",
            Caps::FS_READ => "fs-read",
            Caps::FS_WRITE => "fs-write",
            Caps::SHELL => "shell",
            Caps::NETWORK => "network",
            Caps::ENV_READ => "env-read",
            Caps::ENV_WRITE => "env-write",
            Caps::PROCESS => "process",
            Caps::LLM => "llm",
            Caps::SERIAL => "serial",
            Caps::ALL => "all",
            Caps::STRICT => "strict",
            _ => "unknown",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Caps::NONE),
            "fs-read" => Some(Caps::FS_READ),
            "fs-write" => Some(Caps::FS_WRITE),
            "shell" => Some(Caps::SHELL),
            "network" => Some(Caps::NETWORK),
            "env-read" => Some(Caps::ENV_READ),
            "env-write" => Some(Caps::ENV_WRITE),
            "process" => Some(Caps::PROCESS),
            "llm" => Some(Caps::LLM),
            "serial" => Some(Caps::SERIAL),
            "all" => Some(Caps::ALL),
            "strict" => Some(Caps::STRICT),
            _ => None,
        }
    }
}

impl fmt::Display for Caps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone)]
pub struct Sandbox {
    pub denied: Caps,
    allowed_paths: Option<Vec<crate::path::PathBoundary>>,
    allowed_path_errors: Vec<(PathBuf, String)>,
}

impl Sandbox {
    pub fn allow_all() -> Self {
        Sandbox {
            denied: Caps::NONE,
            allowed_paths: None,
            allowed_path_errors: Vec::new(),
        }
    }

    pub fn deny(caps: Caps) -> Self {
        Sandbox {
            denied: caps,
            allowed_paths: None,
            allowed_path_errors: Vec::new(),
        }
    }

    pub fn with_more_denied(mut self, caps: Caps) -> Self {
        self.denied = self.denied.union(caps);
        self
    }

    pub fn with_allowed_paths(mut self, paths: Vec<PathBuf>) -> Self {
        let mut boundaries = Vec::with_capacity(paths.len());
        self.allowed_path_errors.clear();
        for path in paths {
            match crate::path::PathBoundary::new(&path) {
                Ok(boundary) => boundaries.push(boundary),
                Err(error) => self.allowed_path_errors.push((path, error.to_string())),
            }
        }
        self.allowed_paths = Some(boundaries);
        self
    }

    pub fn is_unrestricted(&self) -> bool {
        self.denied == Caps::NONE && self.allowed_paths.is_none()
    }

    pub fn check(&self, required: Caps, fn_name: &str) -> Result<(), SemaError> {
        // Requesting zero capabilities always succeeds. Without this guard,
        // `denied.contains(Caps::NONE)` is vacuously true, so a restricted
        // sandbox would wrongly deny a `Caps::NONE` request (CORE-3).
        if required == Caps::NONE {
            return Ok(());
        }
        if self.denied.contains(required) {
            Err(SemaError::PermissionDenied {
                function: fn_name.to_string(),
                capability: required.name().to_string(),
            })
        } else {
            Ok(())
        }
    }

    // NOTE: check_path validates the path before the file operation, not during it.
    // This means a TOCTOU (time-of-check-to-time-of-use) window exists where an external
    // process could swap a symlink between the check and the actual fs operation. Mitigating
    // this properly requires OS-specific secure open patterns (openat with O_NOFOLLOW, dirfds)
    // which is a significantly larger change. The current approach is on par with most
    // scripting language sandboxes.
    pub fn check_path(&self, path: &str, fn_name: &str) -> Result<(), SemaError> {
        let allowed = match &self.allowed_paths {
            Some(paths) => paths,
            None => return Ok(()),
        };
        let p = std::path::Path::new(path);
        let mut resolution_error = None;
        for boundary in allowed {
            match boundary.contains(p) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    resolution_error.get_or_insert(error);
                }
            }
        }
        if let Some((root, error)) = self.allowed_path_errors.first() {
            return Err(SemaError::eval(format!(
                "cannot resolve allowed path {}: {error}",
                root.display()
            )));
        }
        if let Some(error) = resolution_error {
            return Err(SemaError::eval(format!(
                "cannot resolve path {} against the allowed paths: {error}",
                p.display()
            )));
        }
        Err(SemaError::PathDenied {
            function: fn_name.to_string(),
            path: p
                .resolve_allow_missing()
                .unwrap_or_else(|_| p.to_path_buf())
                .display()
                .to_string(),
        })
    }

    pub fn parse_allowed_paths(value: &str) -> Vec<PathBuf> {
        value
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    pub fn parse_cli(value: &str) -> Result<Self, String> {
        match value {
            "strict" => Ok(Sandbox::deny(Caps::STRICT)),
            "all" => Ok(Sandbox::deny(Caps::ALL)),
            other => {
                let mut denied = Caps::NONE;
                for part in other.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    let name = part.strip_prefix("no-").unwrap_or(part);
                    match Caps::from_name(name) {
                        Some(cap) => denied = denied.union(cap),
                        None => return Err(format!("unknown capability: {name}")),
                    }
                }
                Ok(Sandbox::deny(denied))
            }
        }
    }
}

impl Default for Sandbox {
    fn default() -> Self {
        Sandbox::allow_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_caps_contains() {
        let all = Caps::FS_READ.union(Caps::FS_WRITE).union(Caps::SHELL);
        assert!(all.contains(Caps::FS_READ));
        assert!(all.contains(Caps::FS_WRITE));
        assert!(all.contains(Caps::SHELL));
        assert!(!all.contains(Caps::NETWORK));
        assert!(!all.contains(Caps::LLM));
    }

    #[test]
    fn test_caps_contains_none_is_always_true() {
        // NONE.contains(NONE) = true since (0 & 0 == 0), but this should not
        // cause false positives in Sandbox::check because check guards against NONE.
        assert!(Caps::NONE.contains(Caps::NONE));
        assert!(Caps::ALL.contains(Caps::NONE));
    }

    #[test]
    fn test_caps_union() {
        let combined = Caps::SHELL.union(Caps::NETWORK);
        assert!(combined.contains(Caps::SHELL));
        assert!(combined.contains(Caps::NETWORK));
        assert!(!combined.contains(Caps::FS_READ));
    }

    #[test]
    fn test_caps_all_contains_every_cap() {
        assert!(Caps::ALL.contains(Caps::FS_READ));
        assert!(Caps::ALL.contains(Caps::FS_WRITE));
        assert!(Caps::ALL.contains(Caps::SHELL));
        assert!(Caps::ALL.contains(Caps::NETWORK));
        assert!(Caps::ALL.contains(Caps::ENV_READ));
        assert!(Caps::ALL.contains(Caps::ENV_WRITE));
        assert!(Caps::ALL.contains(Caps::PROCESS));
        assert!(Caps::ALL.contains(Caps::LLM));
        assert!(Caps::ALL.contains(Caps::SERIAL));
    }

    #[test]
    fn test_caps_strict_preset() {
        assert!(Caps::STRICT.contains(Caps::SHELL));
        assert!(Caps::STRICT.contains(Caps::FS_WRITE));
        assert!(Caps::STRICT.contains(Caps::NETWORK));
        assert!(Caps::STRICT.contains(Caps::ENV_WRITE));
        assert!(Caps::STRICT.contains(Caps::PROCESS));
        assert!(Caps::STRICT.contains(Caps::LLM));
        assert!(Caps::STRICT.contains(Caps::SERIAL));
        // strict does NOT deny read-only operations
        assert!(!Caps::STRICT.contains(Caps::FS_READ));
        assert!(!Caps::STRICT.contains(Caps::ENV_READ));
    }

    #[test]
    fn test_caps_name_roundtrip() {
        let caps = [
            Caps::FS_READ,
            Caps::FS_WRITE,
            Caps::SHELL,
            Caps::NETWORK,
            Caps::ENV_READ,
            Caps::ENV_WRITE,
            Caps::PROCESS,
            Caps::LLM,
            Caps::SERIAL,
        ];
        for cap in caps {
            let name = cap.name();
            assert_eq!(
                Caps::from_name(name),
                Some(cap),
                "roundtrip failed for {name}"
            );
        }
    }

    #[test]
    fn test_caps_from_name_unknown() {
        assert_eq!(Caps::from_name("garbage"), None);
        assert_eq!(Caps::from_name(""), None);
    }

    #[test]
    fn test_caps_display() {
        assert_eq!(format!("{}", Caps::SHELL), "shell");
        assert_eq!(format!("{}", Caps::NETWORK), "network");
        assert_eq!(format!("{}", Caps::FS_READ), "fs-read");
        assert_eq!(format!("{}", Caps::SERIAL), "serial");
    }

    #[test]
    fn test_sandbox_allow_all_is_unrestricted() {
        let sb = Sandbox::allow_all();
        assert!(sb.is_unrestricted());
    }

    #[test]
    fn test_sandbox_deny_is_restricted() {
        let sb = Sandbox::deny(Caps::SHELL);
        assert!(!sb.is_unrestricted());
    }

    #[test]
    fn test_sandbox_default_is_unrestricted() {
        let sb = Sandbox::default();
        assert!(sb.is_unrestricted());
    }

    #[test]
    fn test_sandbox_check_allowed() {
        let sb = Sandbox::deny(Caps::SHELL);
        assert!(sb.check(Caps::NETWORK, "http/get").is_ok());
        assert!(sb.check(Caps::FS_READ, "file/read").is_ok());
    }

    #[test]
    fn test_sandbox_check_denied() {
        let sb = Sandbox::deny(Caps::SHELL);
        let err = sb.check(Caps::SHELL, "shell").unwrap_err();
        assert!(err.to_string().contains("Permission denied"));
        assert!(err.to_string().contains("shell"));
    }

    #[test]
    fn test_sandbox_check_denied_error_format() {
        let sb = Sandbox::deny(Caps::NETWORK);
        let err = sb.check(Caps::NETWORK, "http/get").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("http/get"),
            "should contain function name: {msg}"
        );
        assert!(
            msg.contains("network"),
            "should contain capability name: {msg}"
        );
    }

    #[test]
    fn test_sandbox_check_multiple_denied() {
        let sb = Sandbox::deny(Caps::SHELL.union(Caps::NETWORK));
        assert!(sb.check(Caps::SHELL, "shell").is_err());
        assert!(sb.check(Caps::NETWORK, "http/get").is_err());
        assert!(sb.check(Caps::FS_READ, "file/read").is_ok());
    }

    #[test]
    fn test_sandbox_parse_cli_strict() {
        let sb = Sandbox::parse_cli("strict").unwrap();
        assert!(sb.check(Caps::SHELL, "shell").is_err());
        assert!(sb.check(Caps::FS_WRITE, "file/write").is_err());
        assert!(sb.check(Caps::NETWORK, "http/get").is_err());
        assert!(sb.check(Caps::SERIAL, "serial/list").is_err());
        // strict allows reads
        assert!(sb.check(Caps::FS_READ, "file/read").is_ok());
        assert!(sb.check(Caps::ENV_READ, "env").is_ok());
    }

    #[test]
    fn test_sandbox_parse_cli_all() {
        let sb = Sandbox::parse_cli("all").unwrap();
        assert!(sb.check(Caps::SHELL, "shell").is_err());
        assert!(sb.check(Caps::FS_READ, "file/read").is_err());
        assert!(sb.check(Caps::ENV_READ, "env").is_err());
        assert!(sb.check(Caps::SERIAL, "serial/list").is_err());
    }

    #[test]
    fn test_sandbox_parse_cli_no_prefix() {
        let sb = Sandbox::parse_cli("no-shell,no-network").unwrap();
        assert!(sb.check(Caps::SHELL, "shell").is_err());
        assert!(sb.check(Caps::NETWORK, "http/get").is_err());
        assert!(sb.check(Caps::FS_READ, "file/read").is_ok());
    }

    #[test]
    fn test_sandbox_parse_cli_without_no_prefix() {
        let sb = Sandbox::parse_cli("shell,network").unwrap();
        assert!(sb.check(Caps::SHELL, "shell").is_err());
        assert!(sb.check(Caps::NETWORK, "http/get").is_err());
        assert!(sb.check(Caps::FS_READ, "file/read").is_ok());
    }

    #[test]
    fn test_sandbox_parse_cli_single() {
        let sb = Sandbox::parse_cli("no-fs-write").unwrap();
        assert!(sb.check(Caps::FS_WRITE, "file/write").is_err());
        assert!(sb.check(Caps::FS_READ, "file/read").is_ok());
    }

    #[test]
    fn test_sandbox_parse_cli_with_spaces() {
        let sb = Sandbox::parse_cli("no-shell, no-network").unwrap();
        assert!(sb.check(Caps::SHELL, "shell").is_err());
        assert!(sb.check(Caps::NETWORK, "http/get").is_err());
    }

    #[test]
    fn test_sandbox_parse_cli_empty_parts() {
        let sb = Sandbox::parse_cli("no-shell,,no-network").unwrap();
        assert!(sb.check(Caps::SHELL, "shell").is_err());
        assert!(sb.check(Caps::NETWORK, "http/get").is_err());
    }

    #[test]
    fn test_sandbox_parse_cli_invalid() {
        assert!(Sandbox::parse_cli("no-bogus").is_err());
        assert!(Sandbox::parse_cli("no-shell,no-bogus").is_err());
    }

    #[test]
    fn test_check_path_none_allows_everything() {
        let sb = Sandbox::allow_all();
        assert!(sb.check_path("/etc/passwd", "file/read").is_ok());
        assert!(sb.check_path("relative.txt", "file/read").is_ok());
    }

    #[test]
    fn test_check_path_inside_allowed_dir() {
        let tmp = std::env::temp_dir();
        let sb = Sandbox::allow_all().with_allowed_paths(vec![tmp.clone()]);
        let test_path = tmp.join("sema-test-file.txt");
        std::fs::write(&test_path, "test").ok();
        assert!(sb
            .check_path(test_path.to_str().unwrap(), "file/read")
            .is_ok());
        let _ = std::fs::remove_file(&test_path);
    }

    #[test]
    fn test_check_path_outside_allowed_dir() {
        let tmp = std::env::temp_dir().join("sema-sandbox-test-dir");
        std::fs::create_dir_all(&tmp).ok();
        let sb = Sandbox::allow_all().with_allowed_paths(vec![tmp.clone()]);
        let result = sb.check_path("/etc/hosts", "file/read");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Permission denied"), "{err}");
        assert!(
            err.to_string().contains("outside allowed directories"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_check_path_traversal_attempt() {
        let tmp = std::env::temp_dir().join("sema-sandbox-traverse");
        std::fs::create_dir_all(&tmp).ok();
        let sb = Sandbox::allow_all().with_allowed_paths(vec![tmp.clone()]);
        let evil = format!("{}/../../../etc/passwd", tmp.display());
        let result = sb.check_path(&evil, "file/read");
        assert!(result.is_err(), "path traversal should be denied");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_check_path_multiple_allowed() {
        let dir_a = std::env::temp_dir().join("sema-sandbox-a");
        let dir_b = std::env::temp_dir().join("sema-sandbox-b");
        std::fs::create_dir_all(&dir_a).ok();
        std::fs::create_dir_all(&dir_b).ok();
        let sb = Sandbox::allow_all().with_allowed_paths(vec![dir_a.clone(), dir_b.clone()]);
        let file_a = dir_a.join("ok.txt");
        std::fs::write(&file_a, "a").ok();
        let file_b = dir_b.join("ok.txt");
        std::fs::write(&file_b, "b").ok();
        assert!(sb.check_path(file_a.to_str().unwrap(), "file/read").is_ok());
        assert!(sb.check_path(file_b.to_str().unwrap(), "file/read").is_ok());
        assert!(sb.check_path("/etc/hosts", "file/read").is_err());
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn test_parse_allowed_paths() {
        let paths = Sandbox::parse_allowed_paths("/tmp, /var");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_parse_allowed_paths_empty_parts() {
        let paths = Sandbox::parse_allowed_paths("/tmp,,/var,");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_with_allowed_paths_makes_restricted() {
        let sb = Sandbox::allow_all().with_allowed_paths(vec![std::path::PathBuf::from("/tmp")]);
        assert!(!sb.is_unrestricted());
    }

    #[test]
    fn test_check_path_nonexistent_component_escape() {
        let tmp = std::env::temp_dir().join("sema-sandbox-escape");
        std::fs::create_dir_all(&tmp).ok();
        let sb = Sandbox::allow_all().with_allowed_paths(vec![tmp.clone()]);
        let evil = format!("{}/nonexistent/../../etc/passwd", tmp.display());
        let result = sb.check_path(&evil, "file/write");
        assert!(
            result.is_err(),
            "nonexistent component escape should be denied"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_check_path_relative_nonexistent_escape() {
        let tmp = std::env::temp_dir().join("sema-sandbox-rel-escape");
        let allowed = tmp.join("allowed");
        std::fs::create_dir_all(&allowed).ok();
        let sb = Sandbox::allow_all().with_allowed_paths(vec![allowed.clone()]);
        let evil = format!("{}/fake/../../../etc/hosts", allowed.display());
        let result = sb.check_path(&evil, "file/write");
        assert!(
            result.is_err(),
            "relative nonexistent escape should be denied"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn allowed_symlink_root_cannot_be_retargeted() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let link = temp.path().join("allowed");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        symlink(&first, &link).unwrap();
        let sandbox = Sandbox::allow_all().with_allowed_paths(vec![link.clone()]);

        std::fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();

        assert!(sandbox
            .check_path(second.join("file").to_str().unwrap(), "file/write")
            .is_err());
        assert!(sandbox
            .check_path(first.join("file").to_str().unwrap(), "file/write")
            .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn invalid_allowed_root_reports_its_resolution_error() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let loop_path = temp.path().join("loop");
        symlink(&loop_path, &loop_path).unwrap();
        let sandbox = Sandbox::allow_all().with_allowed_paths(vec![loop_path.clone()]);

        let error = sandbox
            .check_path(temp.path().join("file").to_str().unwrap(), "file/read")
            .unwrap_err();

        assert!(error.to_string().contains("cannot resolve allowed path"));
        assert!(error.to_string().contains(loop_path.to_str().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn invalid_candidate_reports_its_resolution_error() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let allowed = temp.path().join("allowed");
        let loop_path = allowed.join("loop");
        std::fs::create_dir_all(&allowed).unwrap();
        symlink(&loop_path, &loop_path).unwrap();
        let sandbox = Sandbox::allow_all().with_allowed_paths(vec![allowed]);

        let error = sandbox
            .check_path(loop_path.to_str().unwrap(), "file/read")
            .unwrap_err();

        assert!(error.to_string().contains("cannot resolve path"));
        assert!(error.to_string().contains(loop_path.to_str().unwrap()));
    }
}
