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

/// Read exactly one byte from stdin (raw-mode key input). Returns None on EOF.
///
/// Reads straight from the raw fd, NOT `std::io::stdin()` (an 8 KB `BufReader`).
/// Buffering there pulls a whole escape-sequence burst (e.g. `ESC [ C` for an
/// arrow key) into userspace on the first byte, leaving `select()` /
/// `unix_stdin_ready` — which inspect the kernel fd — blind to the continuation
/// bytes. The decoder would then emit a lone ESC and let `[C` leak through as
/// literal characters. An unbuffered read keeps the two in sync. Retry on EINTR
/// (a SIGWINCH etc. can interrupt a blocking read).
#[cfg(unix)]
fn read_one_byte() -> std::io::Result<Option<u8>> {
    let mut buf = [0u8; 1];
    loop {
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 {
            return Ok(Some(buf[0]));
        } else if n == 0 {
            return Ok(None);
        }
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(e);
    }
}

/// Build a Sema list of active modifier keywords from a modifier bitmask
/// (shift=1, alt=2, ctrl=4, super=8). `None` when empty, so callers omit the
/// `:mods` key entirely — keeping bare keys byte-identical to the legacy path.
#[cfg(unix)]
fn mods_list(bits: u32) -> Option<Value> {
    let mut v = Vec::new();
    if bits & 1 != 0 {
        v.push(Value::keyword("shift"));
    }
    if bits & 2 != 0 {
        v.push(Value::keyword("alt"));
    }
    if bits & 4 != 0 {
        v.push(Value::keyword("ctrl"));
    }
    if bits & 8 != 0 {
        v.push(Value::keyword("super"));
    }
    if v.is_empty() {
        None
    } else {
        Some(Value::list(v))
    }
}

/// Decode an SGR (1006) mouse report body (`<b;x;y` + final `M`/`m`) into
/// `{:kind :mouse :action … :x :y :button :mods}`. Only reached when mouse
/// reporting is enabled (`term/enable-mouse`).
#[cfg(unix)]
fn decode_sgr_mouse(csi: &[u8], final_byte: u8) -> Value {
    let body = &csi[1..csi.len().saturating_sub(1)]; // strip leading '<' + final byte
    let parts: Vec<u32> = std::str::from_utf8(body)
        .unwrap_or("")
        .split(';')
        .map(|s| s.parse::<u32>().unwrap_or(0))
        .collect();
    let b = parts.first().copied().unwrap_or(0);
    let x = parts.get(1).copied().unwrap_or(0) as i64;
    let y = parts.get(2).copied().unwrap_or(0) as i64;

    // Modifier bits in the button byte: shift=4, alt/meta=8, ctrl=16 → 1/2/4.
    let mut mbits = 0u32;
    if b & 4 != 0 {
        mbits |= 1;
    }
    if b & 8 != 0 {
        mbits |= 2;
    }
    if b & 16 != 0 {
        mbits |= 4;
    }

    let base = b & 3;
    let action = if b & 64 != 0 {
        // Wheel: low 2 bits give direction.
        match base {
            0 => "wheel-up",
            1 => "wheel-down",
            2 => "wheel-left",
            _ => "wheel-right",
        }
    } else if b & 32 != 0 {
        "move"
    } else if final_byte == b'm' {
        "release"
    } else {
        "press"
    };

    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("kind"), Value::keyword("mouse"));
    m.insert(Value::keyword("action"), Value::keyword(action));
    m.insert(Value::keyword("x"), Value::int(x));
    m.insert(Value::keyword("y"), Value::int(y));
    m.insert(Value::keyword("button"), Value::int(base as i64));
    if let Some(mods) = mods_list(mbits) {
        m.insert(Value::keyword("mods"), mods);
    }
    Value::map(m)
}

/// Decode a kitty keyboard event body (ends with `u`). Normalizes to the SAME
/// `{:kind :char/:ctrl/:alt/:key}` shapes the legacy path emits so existing
/// consumers work unchanged, adding an optional `:mods` list. We request kitty
/// flags WITHOUT event-types, so no repeat/release events arrive (which would
/// otherwise be processed as duplicate key presses).
#[cfg(unix)]
fn decode_kitty(csi: &[u8]) -> Value {
    let body = std::str::from_utf8(&csi[..csi.len().saturating_sub(1)]).unwrap_or("");
    let mut sections = body.split(';');
    let key_sec = sections.next().unwrap_or("");
    let mods_sec = sections.next().unwrap_or("");
    let text_sec = sections.next().unwrap_or("");

    let cp: u32 = key_sec.split(':').next().unwrap_or("").parse().unwrap_or(0);
    // kitty encodes modifiers as (bitmask + 1); subtract before decoding.
    let mbits = mods_sec
        .split(':')
        .next()
        .unwrap_or("")
        .parse::<u32>()
        .unwrap_or(0)
        .saturating_sub(1);
    // Associated text (flag 16): the definitive character when present.
    let text: Option<String> = {
        let s: String = text_sec
            .split(':')
            .filter_map(|c| c.parse::<u32>().ok())
            .filter_map(char::from_u32)
            .collect();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };

    let cp_char = char::from_u32(cp);
    let mut m = std::collections::BTreeMap::new();
    let named = match cp {
        27 => Some("esc"),
        13 | 10 => Some("enter"),
        9 => Some("tab"),
        127 | 8 => Some("backspace"),
        _ => None,
    };
    if let Some(name) = named {
        m.insert(Value::keyword("kind"), Value::keyword("key"));
        m.insert(Value::keyword("name"), Value::keyword(name));
    } else if mbits & 4 != 0 && cp_char.is_some_and(|c| c.is_ascii_alphabetic()) {
        // Ctrl+letter → {:kind :ctrl :char <lowercase>} (legacy semantics).
        let lower = cp_char.unwrap().to_ascii_lowercase();
        m.insert(Value::keyword("kind"), Value::keyword("ctrl"));
        m.insert(Value::keyword("char"), Value::string(&lower.to_string()));
    } else if mbits & 2 != 0 && mbits & 4 == 0 {
        // Alt+char (no ctrl).
        let ch = text
            .clone()
            .unwrap_or_else(|| cp_char.map(|c| c.to_string()).unwrap_or_default());
        m.insert(Value::keyword("kind"), Value::keyword("alt"));
        m.insert(Value::keyword("char"), Value::string(&ch));
    } else {
        // Printable char: prefer the reported text; else derive from the codepoint,
        // uppercasing an ASCII letter when shift is held but no text was sent
        // (terminals without the text flag, e.g. iTerm2).
        let ch = text.clone().unwrap_or_else(|| match cp_char {
            Some(c) if mbits & 1 != 0 && c.is_ascii_alphabetic() => {
                c.to_ascii_uppercase().to_string()
            }
            Some(c) => c.to_string(),
            None => String::new(),
        });
        m.insert(Value::keyword("kind"), Value::keyword("char"));
        m.insert(Value::keyword("char"), Value::string(&ch));
    }
    if let Some(mods) = mods_list(mbits) {
        m.insert(Value::keyword("mods"), mods);
    }
    Value::map(m)
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
            let last = *csi.last().unwrap_or(&0);
            // Dispatch by shape. Only these two are new; everything else falls
            // through to the legacy table below (byte-identical behavior), since
            // no legacy sequence starts with `<` or ends in `u`.
            //   ESC [ < b;x;y (M|m)  → SGR mouse (only sent when mouse enabled)
            //   ESC [ … u            → kitty keyboard event (only when enabled)
            if csi.first() == Some(&b'<') {
                return Ok(Some(decode_sgr_mouse(&csi, last)));
            }
            if last == b'u' {
                return Ok(Some(decode_kitty(&csi)));
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

/// Best-effort absolute, real-path form for containment checks.
///
/// If the whole path exists, canonicalize it (resolving every symlink). If it
/// doesn't exist yet — the "agent about to write a new file" case — we can't
/// just fall back to a lexical absolute, because that would leave any symlink in
/// the *existing* prefix unresolved and let a path escape its sandbox. Instead
/// we canonicalize the deepest ancestor that DOES exist (resolving symlinks
/// there) and re-append the not-yet-existing tail lexically. The first tail
/// component names an entry that doesn't exist, so it cannot itself be a
/// symlink; the check stays sound. This also fixes the mirror-image false
/// negative (e.g. macOS `/var` → `/private/var`), since base and child now have
/// their existing prefixes canonicalized the same way.
fn resolved_path(p: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let abs = lexical_absolute(path);
    let mut ancestor = abs.as_path();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(mut real) = std::fs::canonicalize(ancestor) {
            for seg in tail.iter().rev() {
                real.push(seg);
            }
            return real;
        }
        match (ancestor.parent(), ancestor.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                ancestor = parent;
            }
            // Reached a root that doesn't canonicalize (unusual); best-effort.
            _ => return abs,
        }
    }
}

/// Offload one blocking `std::fs` operation onto THE I/O pool's blocking tier
/// and yield `AwaitIo`, parking the calling task until the op completes.
///
/// Used by the `file/*` builtins when running inside an `async/spawn`'d task,
/// so a slow read/write (big file, cold media, network mount) doesn't stall
/// sibling tasks. Callers must resolve + validate arguments (arity, types,
/// sandbox caps/paths, VFS hits) on the VM thread FIRST; only `Send` facts
/// cross the thread boundary (`work`'s captured paths/contents out, `T` back).
/// `decode` turns the facts into the result `Value` on the VM thread, exactly
/// like the http/shell pollers. Worker errors must be pre-rendered through
/// [`fs_io_msg`] so the task rejection carries the byte-identical message the
/// sync path's `SemaError::Io` would display.
///
/// No abort hook (`IoHandle::new`): unlike a subprocess or an HTTP round-trip
/// there is nothing meaningful to tear down mid-flight — a file op finishes in
/// bounded time — so cancellation is best-effort: the offloaded op runs to
/// completion and its result is discarded (same policy as the LLM
/// `spawn_blocking` tier).
///
/// Returns `Ok(nil)` after arming the yield signal; the scheduler delivers the
/// real value on resume.
fn fs_offload<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    decode: impl Fn(T) -> Value + 'static,
) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<T, String>>();
    sema_io::io_spawn_blocking(move || {
        let _ = tx.send(work());
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });

    let handle = Rc::new(sema_core::IoHandle::new(move || match rx.try_recv() {
        Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
        Ok(Ok(t)) => sema_core::IoPoll::Ready(Ok(decode(t))),
        Ok(Err(msg)) => sema_core::IoPoll::Ready(Err(msg)),
        Err(TryRecvError::Closed) => {
            sema_core::IoPoll::Ready(Err("file: worker dropped".to_string()))
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

/// Render a file-op failure exactly as the sync path raises it: through
/// `SemaError::Io`'s `Display`. A task that fails on the sync path records
/// `format!("{err}")` as its rejection message, so the offloaded path must
/// pre-render the same form for sync/async rejections to be byte-identical.
fn fs_io_msg(msg: String) -> String {
    SemaError::Io(msg).to_string()
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
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload(
                move || {
                    std::fs::read_to_string(&path)
                        .map_err(|e| fs_io_msg(format!("file/read {path}: {e}")))
                },
                |s| Value::string(&s),
            );
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
        if sema_core::in_async_context() {
            let path = path.to_string();
            let content = content.to_string();
            return fs_offload(
                move || {
                    std::fs::write(&path, &content)
                        .map_err(|e| fs_io_msg(format!("file/write {path}: {e}")))
                },
                |()| Value::nil(),
            );
        }
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
            if sema_core::in_async_context() {
                let path = path.to_string();
                return fs_offload(
                    move || {
                        std::fs::read(&path)
                            .map_err(|e| fs_io_msg(format!("file/read-bytes {path}: {e}")))
                    },
                    Value::bytevector,
                );
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
        fn append_impl(path: &str, content: &str) -> Result<(), String> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| format!("file/append {path}: {e}"))?;
            file.write_all(content.as_bytes())
                .map_err(|e| format!("file/append {path}: {e}"))
        }
        if sema_core::in_async_context() {
            let path = path.to_string();
            let content = content.to_string();
            return fs_offload(
                move || append_impl(&path, &content).map_err(fs_io_msg),
                |()| Value::nil(),
            );
        }
        append_impl(path, content).map_err(SemaError::Io)?;
        Ok(Value::nil())
    });

    crate::register_fn_path_gated(env, sandbox, Caps::FS_WRITE, "file/delete", &[0], |args| {
        check_arity!(args, "file/delete", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload(
                move || {
                    std::fs::remove_file(&path)
                        .map_err(|e| fs_io_msg(format!("file/delete {path}: {e}")))
                },
                |()| Value::nil(),
            );
        }
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
                if sema_core::in_async_context() {
                    let path = path.to_string();
                    return fs_offload(
                        move || {
                            std::fs::read_to_string(&path)
                                .map_err(|e| fs_io_msg(format!("file/read-lines {path}: {e}")))
                        },
                        |content| Value::list(content.lines().map(Value::string).collect()),
                    );
                }
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
        if sema_core::in_async_context() {
            let src = src.to_string();
            let dest = dest.to_string();
            return fs_offload(
                move || {
                    std::fs::copy(&src, &dest)
                        .map(|_| ())
                        .map_err(|e| fs_io_msg(format!("file/copy {src} -> {dest}: {e}")))
                },
                |()| Value::nil(),
            );
        }
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

#[cfg(all(test, unix))]
mod within_tests {
    use super::*;

    fn call(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let f = env
            .get(sema_core::intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        let nf = f.as_native_fn_ref().expect("native fn");
        let ctx = sema_core::EvalContext::new();
        (nf.func)(&ctx, args).expect("path/within? call")
    }

    fn make_env() -> sema_core::Env {
        let env = sema_core::Env::new();
        register(&env, &sema_core::Sandbox::allow_all());
        env
    }

    /// `path/within?` is the sandbox cornerstone: a symlink inside the sandbox
    /// that points outside must NOT let a not-yet-created file under it count as
    /// "within" — the primitive resolves the existing prefix (the symlink) even
    /// when the leaf doesn't exist yet. It must also not reject a genuine new
    /// file inside the sandbox (the mirror-image false negative).
    #[test]
    fn symlink_escape_for_new_file_is_rejected() {
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = base.join(format!("sema-within-{nanos}"));
        let sandbox = root.join("sandbox");
        let secret = root.join("secret");
        std::fs::create_dir_all(&sandbox).unwrap();
        std::fs::create_dir_all(&secret).unwrap();
        std::os::unix::fs::symlink(&secret, sandbox.join("escape")).unwrap();

        let env = make_env();
        let sb = sandbox.to_string_lossy().to_string();

        let escape = format!("{sb}/escape/newfile.txt");
        assert_eq!(
            call(
                &env,
                "path/within?",
                &[Value::string(&sb), Value::string(&escape)]
            ),
            Value::bool(false),
            "symlinked path escaping the sandbox must be rejected"
        );

        let legit = format!("{sb}/newfile.txt");
        assert_eq!(
            call(
                &env,
                "path/within?",
                &[Value::string(&sb), Value::string(&legit)]
            ),
            Value::bool(true),
            "a new file directly inside the sandbox must be accepted"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(all(test, unix))]
mod input_decode_tests {
    use super::*;

    fn kw(m: &Value, k: &str) -> Option<Value> {
        m.as_map_ref()
            .and_then(|mm| mm.get(&Value::keyword(k)).cloned())
    }
    fn is_kw(m: &Value, k: &str, want: &str) -> bool {
        kw(m, k) == Some(Value::keyword(want))
    }
    fn mods_of(m: &Value) -> Vec<String> {
        kw(m, "mods")
            .and_then(|v| v.as_list().map(|l| l.to_vec()))
            .unwrap_or_default()
            .iter()
            .filter_map(|k| k.as_keyword().map(|s| s.to_string()))
            .collect()
    }

    // ── SGR mouse ──
    #[test]
    fn sgr_mouse_left_press_and_release() {
        let press = decode_sgr_mouse(b"<0;15;7M", b'M');
        assert!(is_kw(&press, "kind", "mouse"));
        assert!(is_kw(&press, "action", "press"));
        assert_eq!(kw(&press, "x"), Some(Value::int(15)));
        assert_eq!(kw(&press, "y"), Some(Value::int(7)));
        assert_eq!(kw(&press, "button"), Some(Value::int(0)));
        let rel = decode_sgr_mouse(b"<0;15;7m", b'm');
        assert!(is_kw(&rel, "action", "release"));
    }
    #[test]
    fn sgr_mouse_wheel_and_drag_and_mods() {
        assert!(is_kw(
            &decode_sgr_mouse(b"<64;20;3M", b'M'),
            "action",
            "wheel-up"
        ));
        assert!(is_kw(
            &decode_sgr_mouse(b"<65;20;3M", b'M'),
            "action",
            "wheel-down"
        ));
        assert!(is_kw(
            &decode_sgr_mouse(b"<32;10;10M", b'M'),
            "action",
            "move"
        )); // motion
            // b=20 = 16(ctrl)+4(shift), button 0 press
        let m = decode_sgr_mouse(b"<20;5;5M", b'M');
        assert!(is_kw(&m, "action", "press"));
        assert_eq!(mods_of(&m), vec!["shift", "ctrl"]);
    }

    // ── kitty keyboard ── (normalizes to legacy shapes + optional :mods)
    #[test]
    fn kitty_ctrl_c_is_legacy_ctrl() {
        let m = decode_kitty(b"99;5u"); // 'c', mods raw5-1=4 => ctrl
        assert!(is_kw(&m, "kind", "ctrl"));
        assert_eq!(kw(&m, "char"), Some(Value::string("c")));
        assert_eq!(mods_of(&m), vec!["ctrl"]);
    }
    #[test]
    fn kitty_shift_letter_uppercases_without_text() {
        let m = decode_kitty(b"97;2u"); // 'a', mods raw2-1=1 => shift, no text field
        assert!(is_kw(&m, "kind", "char"));
        assert_eq!(kw(&m, "char"), Some(Value::string("A")));
        assert_eq!(mods_of(&m), vec!["shift"]);
    }
    #[test]
    fn kitty_text_field_wins() {
        let m = decode_kitty(b"97;2;65u"); // 'a', shift, text=65 'A'
        assert_eq!(kw(&m, "char"), Some(Value::string("A")));
    }
    #[test]
    fn kitty_special_keys_and_plain() {
        assert!(is_kw(&decode_kitty(b"13u"), "name", "enter"));
        assert!(is_kw(&decode_kitty(b"9u"), "name", "tab"));
        assert!(is_kw(&decode_kitty(b"27u"), "name", "esc"));
        let plain = decode_kitty(b"113u"); // 'q', no mods
        assert!(is_kw(&plain, "kind", "char"));
        assert_eq!(kw(&plain, "char"), Some(Value::string("q")));
        assert_eq!(kw(&plain, "mods"), None); // bare key: no :mods key
    }
}
