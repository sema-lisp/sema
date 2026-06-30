use std::cell::{Cell, RefCell};
use std::io::BufRead;
use std::io::Read as _;
use std::io::Write as _;

use sema_core::{check_arity, Caps, EvalContext, NativeFn, SemaError, Value, ValueView};

use crate::register_fn;

// Thread-local EOF flag: set when any stdin read returns 0 bytes (EOF)
thread_local! {
    static STDIN_EOF: Cell<bool> = const { Cell::new(false) };
}

// TTY restore-token store (unix only)
#[cfg(unix)]
thread_local! {
    static TTY_STORE: RefCell<std::collections::BTreeMap<i64, libc::termios>> =
        const { RefCell::new(std::collections::BTreeMap::new()) };
    static TTY_COUNTER: Cell<i64> = const { Cell::new(0) };
}

// Returns true if stdin has data ready to read within `timeout_ms` milliseconds (0 = non-blocking).
#[cfg(unix)]
fn unix_stdin_ready(timeout_ms: u64) -> bool {
    unsafe {
        let mut readfds: libc::fd_set = std::mem::zeroed();
        libc::FD_ZERO(&mut readfds);
        libc::FD_SET(libc::STDIN_FILENO, &mut readfds);
        let mut tv = libc::timeval {
            tv_sec: (timeout_ms / 1000) as libc::time_t,
            tv_usec: ((timeout_ms % 1000) * 1000) as libc::suseconds_t,
        };
        libc::select(
            libc::STDIN_FILENO + 1,
            &mut readfds,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut tv,
        ) > 0
    }
}

// Read exactly one byte from stdin. Returns None on EOF.
#[cfg(unix)]
fn read_one_byte() -> std::io::Result<Option<u8>> {
    let mut buf = [0u8; 1];
    match std::io::stdin().read(&mut buf) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(buf[0])),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(None),
        Err(e) => Err(e),
    }
}

// Parse a key event from stdin (assuming raw mode).
// Returns Ok(None) on EOF, Ok(Some(value)) on success.
#[cfg(unix)]
fn parse_key_input() -> Result<Option<Value>, SemaError> {
    let b = match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
        None => return Ok(None),
        Some(b) => b,
    };

    // ESC or escape sequence
    if b == 0x1b {
        if !unix_stdin_ready(50) {
            // Plain ESC key
            let mut m = std::collections::BTreeMap::new();
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword("esc"));
            return Ok(Some(Value::map(m)));
        }
        let b2 = match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
            None => {
                let mut m = std::collections::BTreeMap::new();
                m.insert(Value::keyword("kind"), Value::keyword("key"));
                m.insert(Value::keyword("name"), Value::keyword("esc"));
                return Ok(Some(Value::map(m)));
            }
            Some(b) => b,
        };

        if b2 == b'[' {
            // CSI sequence: read until final byte (0x40–0x7e)
            let mut csi: Vec<u8> = Vec::new();
            loop {
                match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
                    None => break,
                    Some(ch) => {
                        csi.push(ch);
                        if (0x40..=0x7e).contains(&ch) {
                            break;
                        }
                    }
                }
            }
            let name = match csi.as_slice() {
                b"A" => "up",
                b"B" => "down",
                b"C" => "right",
                b"D" => "left",
                b"H" => "home",
                b"F" => "end",
                b"Z" => "shift-tab",
                b"3~" => "delete",
                b"5~" => "page-up",
                b"6~" => "page-down",
                _ => "unknown",
            };
            let mut m = std::collections::BTreeMap::new();
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword(name));
            return Ok(Some(Value::map(m)));
        }

        // ESC O sequences (SS3, e.g. function keys on some terminals)
        if b2 == b'O' {
            let b3 = read_one_byte()
                .map_err(|e| SemaError::Io(format!("io/read-key: {e}")))?
                .unwrap_or(0);
            let name = match b3 {
                b'A' => "up",
                b'B' => "down",
                b'C' => "right",
                b'D' => "left",
                b'H' => "home",
                b'F' => "end",
                b'P' => "f1",
                b'Q' => "f2",
                b'R' => "f3",
                b'S' => "f4",
                _ => "unknown",
            };
            let mut m = std::collections::BTreeMap::new();
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword(name));
            return Ok(Some(Value::map(m)));
        }

        // Alt + char  (ESC followed by a regular character)
        let alt_char = char::from(b2);
        let mut m = std::collections::BTreeMap::new();
        m.insert(Value::keyword("kind"), Value::keyword("alt"));
        m.insert(Value::keyword("char"), Value::string(&alt_char.to_string()));
        return Ok(Some(Value::map(m)));
    }

    // DEL / Backspace (0x7f)
    if b == 0x7f {
        let mut m = std::collections::BTreeMap::new();
        m.insert(Value::keyword("kind"), Value::keyword("key"));
        m.insert(Value::keyword("name"), Value::keyword("backspace"));
        return Ok(Some(Value::map(m)));
    }

    // Control characters (0x00–0x1f, excluding ESC already handled)
    if b < 0x20 {
        match b {
            0x08 => {
                // Ctrl-H = backspace
                let mut m = std::collections::BTreeMap::new();
                m.insert(Value::keyword("kind"), Value::keyword("key"));
                m.insert(Value::keyword("name"), Value::keyword("backspace"));
                return Ok(Some(Value::map(m)));
            }
            0x09 => {
                let mut m = std::collections::BTreeMap::new();
                m.insert(Value::keyword("kind"), Value::keyword("key"));
                m.insert(Value::keyword("name"), Value::keyword("tab"));
                return Ok(Some(Value::map(m)));
            }
            0x0a | 0x0d => {
                let mut m = std::collections::BTreeMap::new();
                m.insert(Value::keyword("kind"), Value::keyword("key"));
                m.insert(Value::keyword("name"), Value::keyword("enter"));
                return Ok(Some(Value::map(m)));
            }
            _ => {
                // 0x01–0x1a → Ctrl-A through Ctrl-Z; map to letter
                let ctrl_char = char::from(b.wrapping_add(0x60));
                let mut m = std::collections::BTreeMap::new();
                m.insert(Value::keyword("kind"), Value::keyword("ctrl"));
                m.insert(
                    Value::keyword("char"),
                    Value::string(&ctrl_char.to_string()),
                );
                return Ok(Some(Value::map(m)));
            }
        }
    }

    // Regular ASCII (0x20–0x7e)
    if b < 0x80 {
        let ch = char::from(b);
        let mut m = std::collections::BTreeMap::new();
        m.insert(Value::keyword("kind"), Value::keyword("char"));
        m.insert(Value::keyword("char"), Value::string(&ch.to_string()));
        return Ok(Some(Value::map(m)));
    }

    // Multi-byte UTF-8 character (b >= 0x80)
    let extra = if b & 0xe0 == 0xc0 {
        1usize
    } else if b & 0xf0 == 0xe0 {
        2
    } else if b & 0xf8 == 0xf0 {
        3
    } else {
        0
    };
    let mut bytes = vec![b];
    for _ in 0..extra {
        // Wait up to 20ms for continuation bytes (handles slow pipes and heavy load)
        if !unix_stdin_ready(20) {
            break;
        }
        match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
            None => break,
            Some(ch) => bytes.push(ch),
        }
    }
    let ch_str = std::str::from_utf8(&bytes)
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "?".to_string());
    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("kind"), Value::keyword("char"));
    m.insert(Value::keyword("char"), Value::string(&ch_str));
    Ok(Some(Value::map(m)))
}

// Shared path-component implementations. Each is registered under both a canonical
// slash-namespaced name and a legacy alias (see Decision #24). All return "" when the
// corresponding component is absent (parent / file_name / extension), matching the
// modern Rust/Node idiom and giving consistent behavior across canonical + legacy names.

fn path_dir_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "path/dir", 1);
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let dir = std::path::Path::new(p)
        .parent()
        .and_then(|d| d.to_str())
        .unwrap_or("");
    Ok(Value::string(dir))
}

fn path_filename_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "path/filename", 1);
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let name = std::path::Path::new(p)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    Ok(Value::string(name))
}

fn path_extension_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "path/extension", 1);
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let ext = std::path::Path::new(p)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    Ok(Value::string(ext))
}

/// Resolve `.`/`..` and make `p` absolute *without* touching the filesystem
/// (so it works for paths that don't exist yet — e.g. a file the agent is about
/// to write). Symlinks are NOT resolved here; use `path/canonicalize` when the
/// path exists and symlink resolution matters.
fn lexical_absolute(p: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = if p.is_absolute() {
        std::path::PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    };
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir => out.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::Prefix(pre) => out.push(pre.as_os_str()),
            Component::Normal(seg) => out.push(seg),
        }
    }
    out
}

/// Best-effort absolute, real-path form: canonicalize when the path exists
/// (resolving symlinks — the safe form for containment checks), else fall back
/// to a lexical absolute so not-yet-created paths still compare sensibly.
fn resolved_path(p: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    std::fs::canonicalize(path).unwrap_or_else(|_| lexical_absolute(path))
}

/// Crate-internal: poll stdin for a key within `ms` and decode it, for
/// `event/select`'s `:key` source. Returns the key event, or `None` if no key
/// is ready (or on non-unix platforms, where raw key input isn't wired).
#[cfg(unix)]
pub(crate) fn poll_key_event(ms: u64) -> Option<Value> {
    if !unix_stdin_ready(ms) {
        return None;
    }
    match parse_key_input() {
        Ok(Some(v)) => Some(v),
        _ => None,
    }
}

#[cfg(not(unix))]
pub(crate) fn poll_key_event(_ms: u64) -> Option<Value> {
    None
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    register_fn(env, "display", |args| {
        let mut output = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                output.push(' ');
            }
            match arg.as_str() {
                Some(s) => output.push_str(s),
                None => output.push_str(&format!("{arg}")),
            }
        }
        sema_core::write_stdout(&output);
        let _ = std::io::stdout().flush();
        Ok(Value::nil())
    });

    register_fn(env, "print", |args| {
        let mut output = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                output.push(' ');
            }
            output.push_str(&format!("{arg}"));
        }
        sema_core::write_stdout(&output);
        let _ = std::io::stdout().flush();
        Ok(Value::nil())
    });

    register_fn(env, "println", |args| {
        let mut output = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                output.push(' ');
            }
            match arg.as_str() {
                Some(s) => output.push_str(s),
                None => output.push_str(&format!("{arg}")),
            }
        }
        output.push('\n');
        sema_core::write_stdout(&output);
        Ok(Value::nil())
    });

    register_fn(env, "pprint", |args| {
        check_arity!(args, "pprint", 1);
        sema_core::write_stdout(&format!("{}\n", sema_core::pretty_print(&args[0], 80)));
        Ok(Value::nil())
    });

    register_fn(env, "newline", |args| {
        check_arity!(args, "newline", 0);
        sema_core::write_stdout("\n");
        Ok(Value::nil())
    });

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/read", &[0], |args| {
        check_arity!(args, "file/read", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if let Some(data) = sema_core::vfs::vfs_read(path) {
            return String::from_utf8(data)
                .map(|s| Value::string(&s))
                .map_err(|e| {
                    SemaError::Io(format!("file/read {path}: invalid UTF-8 in VFS: {e}"))
                });
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| SemaError::Io(format!("file/read {path}: {e}")))?;
        Ok(Value::string(&content))
    });

    crate::register_fn_path_gated(env, sandbox, Caps::FS_WRITE, "file/write", &[0], |args| {
        check_arity!(args, "file/write", 2);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let content = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        std::fs::write(path, content)
            .map_err(|e| SemaError::Io(format!("file/write {path}: {e}")))?;
        Ok(Value::nil())
    });

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/read-bytes",
        &[0],
        |args| {
            check_arity!(args, "file/read-bytes", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            if let Some(data) = sema_core::vfs::vfs_read(path) {
                return Ok(Value::bytevector(data));
            }
            let bytes = std::fs::read(path)
                .map_err(|e| SemaError::Io(format!("file/read-bytes {path}: {e}")))?;
            Ok(Value::bytevector(bytes))
        },
    );

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_WRITE,
        "file/write-bytes",
        &[0],
        |args| {
            check_arity!(args, "file/write-bytes", 2);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let bv = args[1]
                .as_bytevector()
                .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
            std::fs::write(path, bv)
                .map_err(|e| SemaError::Io(format!("file/write-bytes {path}: {e}")))?;
            Ok(Value::nil())
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/exists?", &[0], |args| {
        check_arity!(args, "file/exists?", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if let Some(exists) = sema_core::vfs::vfs_exists(path) {
            if exists {
                return Ok(Value::bool(true));
            }
        }
        Ok(Value::bool(std::path::Path::new(path).exists()))
    });

    register_fn(env, "read-line", |args| {
        check_arity!(args, "read-line", 0);
        let mut input = String::new();
        let n = std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| SemaError::Io(format!("read-line: {e}")))?;
        if n == 0 {
            // EOF: stdin was closed (piped input exhausted or Ctrl-D in raw mode)
            STDIN_EOF.with(|f| f.set(true));
            return Ok(Value::nil());
        }
        // Remove trailing newline
        if input.ends_with('\n') {
            input.pop();
            if input.ends_with('\r') {
                input.pop();
            }
        }
        Ok(Value::string(&input))
    });

    register_fn(env, "read", |args| {
        check_arity!(args, "read", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        sema_reader::read(s)
    });

    register_fn(env, "read-many", |args| {
        check_arity!(args, "read-many", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let exprs = sema_reader::read_many(s)?;
        Ok(Value::list(exprs))
    });

    register_fn(env, "error", |args| {
        if args.is_empty() {
            return Err(SemaError::eval("error called with no message"));
        }
        let msg = match args[0].as_str() {
            Some(s) => s.to_string(),
            None => args[0].to_string(),
        };
        Err(SemaError::eval(msg))
    });

    crate::register_fn_path_gated(env, sandbox, Caps::FS_WRITE, "file/append", &[0], |args| {
        check_arity!(args, "file/append", 2);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let content = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| SemaError::Io(format!("file/append {path}: {e}")))?;
        file.write_all(content.as_bytes())
            .map_err(|e| SemaError::Io(format!("file/append {path}: {e}")))?;
        Ok(Value::nil())
    });

    crate::register_fn_path_gated(env, sandbox, Caps::FS_WRITE, "file/delete", &[0], |args| {
        check_arity!(args, "file/delete", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        std::fs::remove_file(path)
            .map_err(|e| SemaError::Io(format!("file/delete {path}: {e}")))?;
        Ok(Value::nil())
    });

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_WRITE,
        "file/rename",
        &[0, 1],
        |args| {
            check_arity!(args, "file/rename", 2);
            let from = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let to = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            std::fs::rename(from, to)
                .map_err(|e| SemaError::Io(format!("file/rename {from} -> {to}: {e}")))?;
            Ok(Value::nil())
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/list", &[0], |args| {
        check_arity!(args, "file/list", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut entries = Vec::new();
        for entry in
            std::fs::read_dir(path).map_err(|e| SemaError::Io(format!("file/list {path}: {e}")))?
        {
            let entry = entry.map_err(|e| SemaError::Io(format!("file/list {path}: {e}")))?;
            entries.push(Value::string(&entry.file_name().to_string_lossy()));
        }
        Ok(Value::list(entries))
    });

    crate::register_fn_path_gated(env, sandbox, Caps::FS_WRITE, "file/mkdir", &[0], |args| {
        check_arity!(args, "file/mkdir", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        std::fs::create_dir_all(path)
            .map_err(|e| SemaError::Io(format!("file/mkdir {path}: {e}")))?;
        Ok(Value::nil())
    });

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/is-directory?",
        &[0],
        |args| {
            check_arity!(args, "file/is-directory?", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::bool(std::path::Path::new(path).is_dir()))
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/is-file?", &[0], |args| {
        check_arity!(args, "file/is-file?", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::bool(std::path::Path::new(path).is_file()))
    });

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/is-symlink?",
        &[0],
        |args| {
            check_arity!(args, "file/is-symlink?", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::bool(std::path::Path::new(path).is_symlink()))
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/info", &[0], |args| {
        check_arity!(args, "file/info", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let meta =
            std::fs::metadata(path).map_err(|e| SemaError::Io(format!("file/info {path}: {e}")))?;
        let mut map = std::collections::BTreeMap::new();
        map.insert(Value::keyword("size"), Value::int(meta.len() as i64));
        map.insert(Value::keyword("is-dir"), Value::bool(meta.is_dir()));
        map.insert(Value::keyword("is-file"), Value::bool(meta.is_file()));
        if let Ok(modified) = meta.modified() {
            if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                map.insert(
                    Value::keyword("modified"),
                    Value::int(duration.as_millis() as i64),
                );
            }
        }
        Ok(Value::map(map))
    });

    register_fn(env, "path/join", |args| {
        check_arity!(args, "path/join", 1..);
        let mut path = std::path::PathBuf::new();
        for arg in args {
            let s = arg
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", arg.type_name()))?;
            path.push(s);
        }
        Ok(Value::string(&path.to_string_lossy()))
    });

    // path/dirname is registered below as an alias of path/dir.
    // path/basename is registered below as an alias of path/filename.
    // path/extension is registered below as an alias of path/ext (canonical name: path/extension).

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "path/absolute", &[0], |args| {
        check_arity!(args, "path/absolute", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let abs = std::fs::canonicalize(s)
            .map_err(|e| SemaError::Io(format!("path/absolute {s}: {e}")))?;
        Ok(Value::string(&abs.to_string_lossy()))
    });

    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "file/glob", |args| {
        check_arity!(args, "file/glob", 1);
        let pattern = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let paths = glob::glob(pattern)
            .map_err(|e| SemaError::eval(format!("file/glob: invalid pattern: {e}")))?;
        let mut items = Vec::new();
        for entry in paths {
            let path = entry.map_err(|e| SemaError::Io(format!("file/glob: {e}")))?;
            items.push(Value::string(path.to_str().unwrap_or("")));
        }
        Ok(Value::list(items))
    });

    // path/extension (canonical) + path/ext (legacy alias) — both return "" for no extension.
    register_fn(env, "path/extension", path_extension_impl);
    register_fn(env, "path/ext", path_extension_impl);

    register_fn(env, "path/stem", |args| {
        check_arity!(args, "path/stem", 1);
        let p = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let stem = std::path::Path::new(p)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        Ok(Value::string(stem))
    });

    // path/dir (canonical) + path/dirname (legacy alias) — both return "" for no parent.
    register_fn(env, "path/dir", path_dir_impl);
    register_fn(env, "path/dirname", path_dir_impl);

    // path/filename (canonical) + path/basename (legacy alias) — both return "" when no file name.
    register_fn(env, "path/filename", path_filename_impl);
    register_fn(env, "path/basename", path_filename_impl);

    register_fn(env, "path/absolute?", |args| {
        check_arity!(args, "path/absolute?", 1);
        let p = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::bool(std::path::Path::new(p).is_absolute()))
    });

    // path/canonicalize — resolve symlinks + `.`/`..` to a real absolute path.
    // Errors if the path doesn't exist (that's what makes it the safe form for
    // checking where a path *actually* points).
    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "path/canonicalize",
        &[0],
        |args| {
            check_arity!(args, "path/canonicalize", 1);
            let s = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let c = std::fs::canonicalize(s)
                .map_err(|e| SemaError::Io(format!("path/canonicalize {s}: {e}")))?;
            Ok(Value::string(&c.to_string_lossy()))
        },
    );

    // path/relative-to — express PATH relative to BASE (pure path math, no fs).
    // (path/relative-to base path) → e.g. base "/a/b", path "/a/b/c/d" → "c/d".
    register_fn(env, "path/relative-to", |args| {
        check_arity!(args, "path/relative-to", 2);
        let base = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let target = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let base = lexical_absolute(std::path::Path::new(base));
        let target = lexical_absolute(std::path::Path::new(target));
        let bc: Vec<_> = base.components().collect();
        let tc: Vec<_> = target.components().collect();
        let mut i = 0;
        while i < bc.len() && i < tc.len() && bc[i] == tc[i] {
            i += 1;
        }
        let mut rel = std::path::PathBuf::new();
        for _ in i..bc.len() {
            rel.push("..");
        }
        for c in &tc[i..] {
            rel.push(c.as_os_str());
        }
        if rel.as_os_str().is_empty() {
            rel.push(".");
        }
        Ok(Value::string(&rel.to_string_lossy()))
    });

    // path/within? — is CHILD contained inside (or equal to) BASE? Resolves
    // symlinks via canonicalize when paths exist, so it catches `../` escapes
    // and symlink escapes. The cornerstone of agent path sandboxing.
    register_fn(env, "path/within?", |args| {
        check_arity!(args, "path/within?", 2);
        let base = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let child = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let base = resolved_path(base);
        let child = resolved_path(child);
        Ok(Value::bool(child.starts_with(&base)))
    });

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/read-lines",
        &[0],
        |args| {
            check_arity!(args, "file/read-lines", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let content = if let Some(data) = sema_core::vfs::vfs_read(path) {
                String::from_utf8(data).map_err(|e| {
                    SemaError::Io(format!("file/read-lines {path}: invalid UTF-8 in VFS: {e}"))
                })?
            } else {
                std::fs::read_to_string(path)
                    .map_err(|e| SemaError::Io(format!("file/read-lines {path}: {e}")))?
            };
            let lines: Vec<Value> = content.lines().map(Value::string).collect();
            Ok(Value::list(lines))
        },
    );

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/for-each-line",
        &[0],
        |args| {
            check_arity!(args, "file/for-each-line", 2);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let func = args[1].clone();
            let file = std::fs::File::open(path)
                .map_err(|e| SemaError::Io(format!("file/for-each-line {path}: {e}")))?;
            let mut reader = std::io::BufReader::new(file);

            sema_core::with_stdlib_ctx(|ctx| {
                let mut line_buf = String::with_capacity(64);
                loop {
                    line_buf.clear();
                    let n = reader
                        .read_line(&mut line_buf)
                        .map_err(|e| SemaError::Io(format!("file/for-each-line {path}: {e}")))?;
                    if n == 0 {
                        break;
                    }
                    if line_buf.ends_with('\n') {
                        line_buf.pop();
                        if line_buf.ends_with('\r') {
                            line_buf.pop();
                        }
                    }
                    sema_core::call_callback(ctx, &func, &[Value::string(&line_buf)])?;
                }
                Ok(Value::nil())
            })
        },
    );

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/fold-lines",
        &[0],
        |args| {
            check_arity!(args, "file/fold-lines", 3);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let func = args[1].clone();
            let mut acc = args[2].clone();
            let file = std::fs::File::open(path)
                .map_err(|e| SemaError::Io(format!("file/fold-lines {path}: {e}")))?;
            // 256KB buffer (vs default 8KB) improves throughput for large file reads.
            let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);

            sema_core::with_stdlib_ctx(|ctx| {
                let mut line_buf = String::with_capacity(64);
                // Fast path: if the callback is a NativeFn, call it directly.
                // This avoids the call_callback indirection and, critically, avoids
                // the VM closure fallback wrapper's clone of args (which prevents
                // COW optimizations in functions like assoc).
                #[allow(clippy::type_complexity)]
                let native: Option<
                    &dyn Fn(&EvalContext, &[Value]) -> Result<Value, SemaError>,
                > = func.as_native_fn_ref().map(|n| &*n.func);
                loop {
                    line_buf.clear();
                    let n = reader
                        .read_line(&mut line_buf)
                        .map_err(|e| SemaError::Io(format!("file/fold-lines {path}: {e}")))?;
                    if n == 0 {
                        break;
                    }
                    if line_buf.ends_with('\n') {
                        line_buf.pop();
                        if line_buf.ends_with('\r') {
                            line_buf.pop();
                        }
                    }
                    let line_val = Value::string(&line_buf);
                    let args = [std::mem::replace(&mut acc, Value::nil()), line_val];
                    acc = if let Some(f) = native {
                        f(ctx, &args)?
                    } else {
                        sema_core::call_callback(ctx, &func, &args)?
                    };
                }
                Ok(acc)
            })
        },
    );

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_WRITE,
        "file/write-lines",
        &[0],
        |args| {
            check_arity!(args, "file/write-lines", 2);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let lines = match args[1].view() {
                ValueView::List(l) => l,
                ValueView::Vector(v) => v,
                _ => return Err(SemaError::type_error("list or vector", args[1].type_name())),
            };
            let strs: Vec<String> = lines
                .iter()
                .map(|v| match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(),
                })
                .collect();
            let content = strs.join("\n");
            std::fs::write(path, content)
                .map_err(|e| SemaError::Io(format!("file/write-lines {path}: {e}")))?;
            Ok(Value::nil())
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_WRITE, "file/copy", &[0, 1], |args| {
        check_arity!(args, "file/copy", 2);
        let src = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let dest = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        std::fs::copy(src, dest)
            .map_err(|e| SemaError::Io(format!("file/copy {src} -> {dest}: {e}")))?;
        Ok(Value::nil())
    });

    register_fn(env, "print-error", |args| {
        let mut output = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                output.push(' ');
            }
            match arg.as_str() {
                Some(s) => output.push_str(s),
                None => output.push_str(&format!("{arg}")),
            }
        }
        sema_core::write_stderr(&output);
        std::io::stderr().flush().ok();
        Ok(Value::nil())
    });

    register_fn(env, "println-error", |args| {
        let mut output = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                output.push(' ');
            }
            match arg.as_str() {
                Some(s) => output.push_str(s),
                None => output.push_str(&format!("{arg}")),
            }
        }
        output.push('\n');
        sema_core::write_stderr(&output);
        Ok(Value::nil())
    });

    register_fn(env, "read-stdin", |args| {
        check_arity!(args, "read-stdin", 0);
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| SemaError::Io(format!("read-stdin: {e}")))?;
        STDIN_EOF.with(|f| f.set(true));
        Ok(Value::string(&buf))
    });

    // io/flush — flush stdout
    register_fn(env, "io/flush", |args| {
        check_arity!(args, "io/flush", 0);
        std::io::stdout()
            .flush()
            .map_err(|e| SemaError::Io(format!("io/flush: {e}")))?;
        Ok(Value::nil())
    });

    // io/eof? — true after stdin has returned EOF (set by read-line / read-stdin / read-key returning nil)
    register_fn(env, "io/eof?", |args| {
        check_arity!(args, "io/eof?", 0);
        Ok(Value::bool(STDIN_EOF.with(|f| f.get())))
    });

    // ─── TTY / raw-mode + keystroke reader (Unix only) ───────────────────────
    #[cfg(unix)]
    {
        use std::io::IsTerminal;

        // io/tty-raw! — put the controlling TTY into raw mode.
        // Returns a restore-token (integer) on success, nil if stdin is not a TTY.
        register_fn(env, "io/tty-raw!", |args| {
            check_arity!(args, "io/tty-raw!", 0);
            if !std::io::stdin().is_terminal() {
                return Ok(Value::nil());
            }
            // SAFETY: termios is a POD C struct; zero-init is the standard idiom.
            // tcgetattr (called next) overwrites if successful; we short-circuit if it fails.
            let mut orig: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut orig) } != 0 {
                return Ok(Value::nil());
            }
            let id = TTY_COUNTER.with(|c| {
                let n = c.get();
                c.set(n + 1);
                n
            });
            TTY_STORE.with(|s| s.borrow_mut().insert(id, orig));
            let mut raw = orig;
            unsafe { libc::cfmakeraw(&mut raw) };
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
                return Err(SemaError::eval(format!(
                    "io/tty-raw!: tcsetattr failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(Value::int(id))
        });

        // io/tty-restore! — restore the TTY to cooked mode using the given restore-token.
        register_fn(env, "io/tty-restore!", |args| {
            check_arity!(args, "io/tty-restore!", 1);
            let id = args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
            TTY_STORE.with(|s| {
                if let Some(orig) = s.borrow_mut().remove(&id) {
                    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &orig) } != 0 {
                        eprintln!(
                            "io/tty-restore!: tcsetattr failed: {}",
                            std::io::Error::last_os_error()
                        );
                    }
                }
            });
            Ok(Value::nil())
        });

        // io/read-key — blocking; reads one keypress and returns a map describing it.
        // Returns nil on EOF.
        // Map shapes:
        //   {:kind :char   :char "a"}         regular printable character
        //   {:kind :ctrl   :char "c"}         Ctrl-C (ctrl + letter)
        //   {:kind :key    :name :enter}      named key (:enter :backspace :tab :esc :up …)
        //   {:kind :alt    :char "x"}         Alt + character
        register_fn(env, "io/read-key", |args| {
            check_arity!(args, "io/read-key", 0);
            match parse_key_input()? {
                None => {
                    STDIN_EOF.with(|f| f.set(true));
                    Ok(Value::nil())
                }
                Some(v) => Ok(v),
            }
        });

        // io/read-key-timeout — like io/read-key but returns nil if no key arrives within
        // `timeout-ms` milliseconds.
        register_fn(env, "io/read-key-timeout", |args| {
            check_arity!(args, "io/read-key-timeout", 1);
            let ms = args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?
                as u64;
            if !unix_stdin_ready(ms) {
                return Ok(Value::nil());
            }
            match parse_key_input()? {
                None => {
                    STDIN_EOF.with(|f| f.set(true));
                    Ok(Value::nil())
                }
                Some(v) => Ok(v),
            }
        });
    }

    // module/function aliases for legacy names
    if let Some(v) = env.get(sema_core::intern("read-line")) {
        env.set(sema_core::intern("io/read-line"), v);
    }
    if let Some(v) = env.get(sema_core::intern("read-many")) {
        env.set(sema_core::intern("io/read-many"), v);
    }
    if let Some(v) = env.get(sema_core::intern("read-stdin")) {
        env.set(sema_core::intern("io/read-stdin"), v);
    }
    if let Some(v) = env.get(sema_core::intern("print-error")) {
        env.set(sema_core::intern("io/print-error"), v);
    }
    if let Some(v) = env.get(sema_core::intern("println-error")) {
        env.set(sema_core::intern("io/println-error"), v);
    }

    register_log_fn(env, "log/info", "INFO");
    register_log_fn(env, "log/warn", "WARN");
    register_log_fn(env, "log/error", "ERROR");
    register_log_fn(env, "log/debug", "DEBUG");

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "load", &[0], |args| {
        check_arity!(args, "load", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let content = std::fs::read_to_string(path)
            .map_err(|e| SemaError::Io(format!("load {path}: {e}")))?;
        // Parse and return as a list of expressions for the caller to eval
        let exprs = sema_reader::read_many(&content)?;
        Ok(Value::list(exprs))
    });
}

fn register_log_fn(env: &sema_core::Env, name: &str, level: &'static str) {
    let fn_name = name.to_string();
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, move |ctx, args| {
            check_arity!(args, &fn_name, 1..);
            let mut msg = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    msg.push(' ');
                }
                match arg.as_str() {
                    Some(s) => msg.push_str(s),
                    None => msg.push_str(&arg.to_string()),
                }
            }
            let context = ctx.context_all();
            if context.is_empty() {
                eprintln!("[{level}] {msg}");
            } else {
                eprintln!("[{level}] {msg} {}", Value::map(context));
            }
            Ok(Value::nil())
        })),
    );
}
