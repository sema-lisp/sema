use std::cell::{Cell, RefCell};
use std::io::BufRead;
use std::io::Read as _;
use std::io::Write as _;

use sema_core::{check_arity, Caps, NativeFn, SemaError, Value, ValueView};

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
    // Count of outstanding DSR (cursor-position) queries. A `CSI…R` is a cursor
    // report only when one is pending; otherwise it's modified-F3 keyboard input
    // (`CSI 1;<mod>R`), which is byte-identical to a CPR reply.
    static EXPECT_CPR: Cell<u32> = const { Cell::new(0) };
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
    // Extended kitty modifiers (only reported under the kitty protocol).
    if bits & 16 != 0 {
        v.push(Value::keyword("hyper"));
    }
    if bits & 32 != 0 {
        v.push(Value::keyword("meta"));
    }
    if bits & 64 != 0 {
        v.push(Value::keyword("caps-lock"));
    }
    if bits & 128 != 0 {
        v.push(Value::keyword("num-lock"));
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

    // key section: unicode-codepoint[:shifted-codepoint[:base-layout-codepoint]]
    // (the alternates arrive only under the "report alternate keys" flag).
    let mut key_parts = key_sec.split(':');
    let cp: u32 = key_parts.next().unwrap_or("").parse().unwrap_or(0);
    let shifted_cp = key_parts.next().and_then(|s| s.parse::<u32>().ok());
    let base_cp = key_parts.next().and_then(|s| s.parse::<u32>().ok());
    // mods section: modifiers[:event-type]. kitty encodes modifiers as (bitmask+1).
    let mut mod_parts = mods_sec.split(':');
    let mbits = mod_parts
        .next()
        .unwrap_or("")
        .parse::<u32>()
        .unwrap_or(0)
        .saturating_sub(1);
    // event-type 1=press (default), 2=repeat, 3=release — present only under the
    // "report event types" flag (bit 2).
    let event_type = mod_parts.next().and_then(|s| s.parse::<u32>().ok());
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
    if let Some(et) = event_type {
        m.insert(
            Value::keyword("event"),
            Value::keyword(match et {
                2 => "repeat",
                3 => "release",
                _ => "press",
            }),
        );
    }
    if let Some(sc) = shifted_cp.and_then(char::from_u32) {
        m.insert(
            Value::keyword("shifted-key"),
            Value::string(&sc.to_string()),
        );
    }
    if let Some(bc) = base_cp.and_then(char::from_u32) {
        m.insert(Value::keyword("base-key"), Value::string(&bc.to_string()));
    }
    if let Some(mods) = mods_list(mbits) {
        m.insert(Value::keyword("mods"), mods);
    }
    Value::map(m)
}

/// Parse a legacy CSI body (the bytes after `ESC [`, including the final byte)
/// into a key name and an xterm modifier bitmask. Handles the bare forms
/// (`A`, `3~`, …) and the modified forms `1;<mod><final>` and `<n>;<mod>~`,
/// where `<mod>` is `1 + bitmask` (shift=1, alt=2, ctrl=4, super=8). mbits is 0
/// when no modifier is present, keeping bare keys byte-identical to before.
#[cfg(unix)]
fn parse_legacy_csi(csi: &[u8]) -> (&'static str, u32) {
    let last = *csi.last().unwrap_or(&0);
    let params = &csi[..csi.len().saturating_sub(1)];
    let mut fields = params.split(|&b| b == b';');
    let first = fields.next().unwrap_or(b"");
    let mbits = fields
        .next()
        .and_then(|f| std::str::from_utf8(f).ok())
        .and_then(|s| s.parse::<u32>().ok())
        .map(|m| m.saturating_sub(1))
        .unwrap_or(0);
    let name = match last {
        b'A' => "up",
        b'B' => "down",
        b'C' => "right",
        b'D' => "left",
        b'H' => "home",
        b'F' => "end",
        b'Z' => "shift-tab",
        // Modified F1-F4 arrive as CSI `1;<mod>P/Q/R/S`. F3's `…R` is byte-shaped
        // like a cursor-position report; the dispatcher only reaches here for `R`
        // when no DSR reply is outstanding, so it's genuinely modified-F3.
        b'P' => "f1",
        b'Q' => "f2",
        b'R' => "f3",
        b'S' => "f4",
        b'~' => match first {
            b"1" | b"7" => "home",
            b"2" => "insert",
            b"3" => "delete",
            b"4" | b"8" => "end",
            b"5" => "page-up",
            b"6" => "page-down",
            b"11" => "f1",
            b"12" => "f2",
            b"13" => "f3",
            b"14" => "f4",
            b"15" => "f5",
            b"17" => "f6",
            b"18" => "f7",
            b"19" => "f8",
            b"20" => "f9",
            b"21" => "f10",
            b"23" => "f11",
            b"24" => "f12",
            _ => "unknown",
        },
        _ => "unknown",
    };
    (name, mbits)
}

#[cfg(all(test, unix))]
mod legacy_csi_tests {
    use super::parse_legacy_csi;

    #[test]
    fn bare_sequences_are_unmodified() {
        assert_eq!(parse_legacy_csi(b"C"), ("right", 0));
        assert_eq!(parse_legacy_csi(b"D"), ("left", 0));
        assert_eq!(parse_legacy_csi(b"3~"), ("delete", 0));
        assert_eq!(parse_legacy_csi(b"5~"), ("page-up", 0));
        assert_eq!(parse_legacy_csi(b"Z"), ("shift-tab", 0));
    }

    #[test]
    fn modified_arrows_carry_the_modifier_bitmask() {
        // xterm modparam = 1 + bitmask (shift=1, alt=2, ctrl=4).
        assert_eq!(parse_legacy_csi(b"1;3C"), ("right", 2)); // alt (Option)
        assert_eq!(parse_legacy_csi(b"1;3D"), ("left", 2)); // alt (Option)
        assert_eq!(parse_legacy_csi(b"1;5C"), ("right", 4)); // ctrl
        assert_eq!(parse_legacy_csi(b"3;3~"), ("delete", 2)); // alt+delete
    }

    #[test]
    fn function_and_nav_keys() {
        assert_eq!(parse_legacy_csi(b"2~"), ("insert", 0));
        assert_eq!(parse_legacy_csi(b"1~"), ("home", 0));
        assert_eq!(parse_legacy_csi(b"7~"), ("home", 0));
        assert_eq!(parse_legacy_csi(b"4~"), ("end", 0));
        assert_eq!(parse_legacy_csi(b"8~"), ("end", 0));
        assert_eq!(parse_legacy_csi(b"15~"), ("f5", 0));
        assert_eq!(parse_legacy_csi(b"21~"), ("f10", 0));
        assert_eq!(parse_legacy_csi(b"24~"), ("f12", 0));
        assert_eq!(parse_legacy_csi(b"1;2P"), ("f1", 1)); // shift+F1
        assert_eq!(parse_legacy_csi(b"15;5~"), ("f5", 4)); // ctrl+F5
                                                           // Unsolicited CSI…R reaches parse_legacy_csi as modified-F3 (the
                                                           // dispatcher only routes R to CPR when a DSR reply is outstanding).
        assert_eq!(parse_legacy_csi(b"1;2R"), ("f3", 1)); // shift+F3
    }
}

#[cfg(all(test, unix))]
mod terminal_response_tests {
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

    #[test]
    fn cursor_position_report() {
        let m = decode_cpr(b"12;40R");
        assert!(is_kw(&m, "kind", "cpr"));
        assert_eq!(kw(&m, "row"), Some(Value::int(12)));
        assert_eq!(kw(&m, "col"), Some(Value::int(40)));
        // DECXCPR form `CSI ? row;col R` (leading ? stripped).
        let d = decode_cpr(b"?12;40R");
        assert_eq!(kw(&d, "row"), Some(Value::int(12)));
        assert_eq!(kw(&d, "col"), Some(Value::int(40)));
    }

    #[test]
    fn device_attributes_primary_and_secondary() {
        let p = decode_device_attributes(b"?1;2c", b'?');
        assert!(is_kw(&p, "kind", "device-attributes"));
        assert!(is_kw(&p, "device", "primary"));
        assert!(is_kw(
            &decode_device_attributes(b">0;95;0c", b'>'),
            "device",
            "secondary"
        ));
    }

    #[test]
    fn kitty_flags_reply() {
        let m = decode_kitty_flags(b"?15u");
        assert!(is_kw(&m, "kind", "kitty-flags"));
        assert_eq!(kw(&m, "flags"), Some(Value::int(15)));
    }

    #[test]
    fn modify_other_keys_ctrl_tab() {
        let m = decode_modify_other_keys(b"27;5;9~"); // ctrl + tab(9)
        assert!(is_kw(&m, "kind", "key"));
        assert!(is_kw(&m, "name", "tab"));
        assert_eq!(mods_of(&m), vec!["ctrl"]);
    }

    #[test]
    fn kitty_event_type_and_extended_mods() {
        let rel = decode_kitty(b"97;1:3u"); // 'a', no mods, event 3 = release
        assert!(is_kw(&rel, "kind", "char"));
        assert_eq!(kw(&rel, "char"), Some(Value::string("a")));
        assert!(is_kw(&rel, "event", "release"));
        let sup = decode_kitty(b"97;9u"); // 'a', mods raw9-1=8 = super
        assert_eq!(mods_of(&sup), vec!["super"]);
    }

    #[test]
    fn focus_events() {
        assert!(is_kw(&focus_event(true), "kind", "focus"));
        assert_eq!(kw(&focus_event(true), "focused"), Some(Value::bool(true)));
        assert_eq!(kw(&focus_event(false), "focused"), Some(Value::bool(false)));
    }
}

/// Read the continuation bytes of a UTF-8 character whose lead byte is `lead`,
/// returning the decoded string ("?" on invalid UTF-8). A single-byte ASCII
/// lead returns that char unchanged, so this also covers Alt+ASCII.
#[cfg(unix)]
fn read_utf8_char(lead: u8) -> Result<String, SemaError> {
    let extra = if lead & 0xe0 == 0xc0 {
        1usize
    } else if lead & 0xf0 == 0xe0 {
        2
    } else if lead & 0xf8 == 0xf0 {
        3
    } else {
        0
    };
    let mut bytes = vec![lead];
    for _ in 0..extra {
        // Wait up to 20ms for continuation bytes (handles slow pipes / heavy load).
        if !unix_stdin_ready(20) {
            break;
        }
        match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
            None => break,
            Some(ch) => bytes.push(ch),
        }
    }
    Ok(std::str::from_utf8(&bytes)
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "?".to_string()))
}

/// `{:kind :focus :focused <bool>}` — a terminal focus in/out report (enabled
/// via `term/enable-focus-events`).
#[cfg(unix)]
fn focus_event(focused: bool) -> Value {
    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("kind"), Value::keyword("focus"));
    m.insert(Value::keyword("focused"), Value::bool(focused));
    Value::map(m)
}

/// Split a CSI body (minus the final byte, with an optional leading marker
/// stripped) into its `;`-separated integer parameters.
#[cfg(unix)]
fn csi_params(body: &[u8]) -> Vec<u32> {
    std::str::from_utf8(body)
        .unwrap_or("")
        .split(';')
        .map(|s| s.parse::<u32>().unwrap_or(0))
        .collect()
}

/// `{:kind :cpr :row R :col C}` — a cursor-position report (`ESC[row;colR`),
/// the reply to a DSR `ESC[6n` query.
#[cfg(unix)]
fn decode_cpr(csi: &[u8]) -> Value {
    // Strip the final `R` and an optional leading `?` (DECXCPR: `CSI ? row;col R`).
    let mut body = &csi[..csi.len().saturating_sub(1)];
    if body.first() == Some(&b'?') {
        body = &body[1..];
    }
    let p = csi_params(body);
    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("kind"), Value::keyword("cpr"));
    m.insert(
        Value::keyword("row"),
        Value::int(p.first().copied().unwrap_or(0) as i64),
    );
    m.insert(
        Value::keyword("col"),
        Value::int(p.get(1).copied().unwrap_or(0) as i64),
    );
    Value::map(m)
}

/// `{:kind :device-attributes :device :primary|:secondary :params (…)}` — a DA1
/// (`ESC[?…c`) or DA2 (`ESC[>…c`) reply.
#[cfg(unix)]
fn decode_device_attributes(csi: &[u8], first: u8) -> Value {
    // strip the leading marker (`?` or `>`) and the final `c`.
    let body = &csi[1..csi.len().saturating_sub(1)];
    let params: Vec<Value> = csi_params(body)
        .into_iter()
        .map(|n| Value::int(n as i64))
        .collect();
    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("kind"), Value::keyword("device-attributes"));
    m.insert(
        Value::keyword("device"),
        Value::keyword(if first == b'>' {
            "secondary"
        } else {
            "primary"
        }),
    );
    m.insert(Value::keyword("params"), Value::list(params));
    Value::map(m)
}

/// `{:kind :kitty-flags :flags N}` — the reply to a `CSI ?u` progressive-
/// enhancement query (`ESC[?<flags>u`).
#[cfg(unix)]
fn decode_kitty_flags(csi: &[u8]) -> Value {
    // strip leading `?` and final `u`.
    let body = &csi[1..csi.len().saturating_sub(1)];
    let flags = csi_params(body).first().copied().unwrap_or(0);
    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("kind"), Value::keyword("kitty-flags"));
    m.insert(Value::keyword("flags"), Value::int(flags as i64));
    Value::map(m)
}

/// xterm modifyOtherKeys: `ESC[27;<mod>;<code>~` → the base key `<code>` with
/// its modifiers, normalized to the same shapes the legacy path emits.
#[cfg(unix)]
fn decode_modify_other_keys(csi: &[u8]) -> Value {
    let p = csi_params(&csi[..csi.len().saturating_sub(1)]);
    let mbits = p.get(1).copied().unwrap_or(1).saturating_sub(1);
    let cp = p.get(2).copied().unwrap_or(0);
    let cp_char = char::from_u32(cp);
    let mut m = std::collections::BTreeMap::new();
    match cp {
        27 => {
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword("esc"));
        }
        13 | 10 => {
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword("enter"));
        }
        9 => {
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword("tab"));
        }
        127 | 8 => {
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword("backspace"));
        }
        _ if mbits & 4 != 0 && cp_char.is_some_and(|c| c.is_ascii_alphabetic()) => {
            let lower = cp_char.unwrap().to_ascii_lowercase();
            m.insert(Value::keyword("kind"), Value::keyword("ctrl"));
            m.insert(Value::keyword("char"), Value::string(&lower.to_string()));
        }
        _ => {
            m.insert(Value::keyword("kind"), Value::keyword("char"));
            m.insert(
                Value::keyword("char"),
                Value::string(&cp_char.map(|c| c.to_string()).unwrap_or_default()),
            );
        }
    }
    if let Some(mods) = mods_list(mbits) {
        m.insert(Value::keyword("mods"), mods);
    }
    Value::map(m)
}

/// Collect a bracketed-paste payload: the literal bytes after `ESC[200~` up to
/// the `ESC[201~` terminator, as `{:kind :paste :text …}`. Pasted content
/// bypasses key dispatch entirely, so control bytes in a paste can't be
/// misread as live keystrokes.
#[cfg(unix)]
fn read_bracketed_paste() -> Result<Value, SemaError> {
    const TERMINATOR: &[u8] = b"\x1b[201~";
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
            None => break,
            Some(ch) => {
                buf.push(ch);
                if buf.ends_with(TERMINATOR) {
                    buf.truncate(buf.len() - TERMINATOR.len());
                    break;
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("kind"), Value::keyword("paste"));
    m.insert(Value::keyword("text"), Value::string(&text));
    Ok(Value::map(m))
}

/// True when stdin is a real terminal (probes are meaningless otherwise).
#[cfg(unix)]
fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

/// Write a control sequence to stdout and flush (for query round-trips).
#[cfg(unix)]
fn write_stdout(seq: &str) -> Result<(), SemaError> {
    use std::io::Write;
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())
        .and_then(|_| out.flush())
        .map_err(|e| SemaError::Io(format!("term: {e}")))
}

/// Scan stdin for the next complete CSI sequence (`ESC [ … final`), returning
/// its body (params + final byte) — or None on a `budget_ms` idle timeout.
/// Non-CSI bytes are skipped; used only by the capability probes below.
#[cfg(unix)]
fn read_one_csi(budget_ms: u64) -> Result<Option<Vec<u8>>, SemaError> {
    loop {
        if !unix_stdin_ready(budget_ms) {
            return Ok(None);
        }
        match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
            None => return Ok(None),
            Some(0x1b) => {}
            Some(_) => continue,
        }
        if !unix_stdin_ready(50) {
            return Ok(None);
        }
        match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
            Some(b'[') => {}
            _ => continue,
        }
        let mut csi: Vec<u8> = Vec::new();
        loop {
            match read_one_byte().map_err(|e| SemaError::Io(format!("io/read-key: {e}")))? {
                None => return Ok(None),
                Some(ch) => {
                    csi.push(ch);
                    if (0x40..=0x7e).contains(&ch) {
                        break;
                    }
                }
            }
        }
        return Ok(Some(csi));
    }
}

/// Detect kitty-keyboard support: send the flags query `CSI ?u` followed by a
/// Primary Device Attributes barrier `CSI c` (the method the kitty spec
/// recommends). A supporting terminal answers the flags query (`ESC[?…u`) before
/// the DA reply (`ESC[?…c`); a non-supporting one answers only DA. Must be called
/// in raw mode; returns false when stdin is not a TTY or nothing replies.
/// NOTE: inside tmux, kitty forwarding is off by default and detection may
/// silently fail — prefer letting the user force-enable when `$TMUX` is set.
#[cfg(unix)]
fn probe_kitty_support() -> Result<bool, SemaError> {
    if !stdin_is_tty() {
        return Ok(false);
    }
    write_stdout("\x1b[?u\x1b[c")?;
    while let Some(csi) = read_one_csi(200)? {
        let last = *csi.last().unwrap_or(&0);
        if last == b'u' && csi.first() == Some(&b'?') {
            return Ok(true);
        }
        if last == b'c' {
            return Ok(false); // DA barrier reached with no kitty reply
        }
    }
    Ok(false)
}

/// Round-trip the cursor position: send DSR (`CSI 6n`) and return `{:row :col}`
/// from the reply, or nil (not a TTY / no reply). Must be called in raw mode.
#[cfg(unix)]
fn query_cursor_position() -> Result<Value, SemaError> {
    if !stdin_is_tty() {
        return Ok(Value::nil());
    }
    write_stdout("\x1b[6n")?;
    while let Some(csi) = read_one_csi(200)? {
        if *csi.last().unwrap_or(&0) == b'R' {
            let p = csi_params(&csi[..csi.len().saturating_sub(1)]);
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                Value::keyword("row"),
                Value::int(p.first().copied().unwrap_or(0) as i64),
            );
            m.insert(
                Value::keyword("col"),
                Value::int(p.get(1).copied().unwrap_or(0) as i64),
            );
            return Ok(Value::map(m));
        }
    }
    Ok(Value::nil())
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
            let first = *csi.first().unwrap_or(&0);
            // Dispatch by shape (each form is unambiguous by its marker/final byte):
            //   ESC[<b;x;y M|m  → SGR mouse            ESC[…u        → kitty key
            //   ESC[?…u         → kitty flags reply    ESC[200~      → bracketed paste
            //   ESC[I | ESC[O   → focus in/out         ESC[?…c/>…c   → device attrs
            //   ESC[r;cR        → cursor-position rpt   ESC[27;m;c~   → modifyOtherKeys
            //   else            → legacy keys + xterm modifier forms + function keys
            if first == b'<' {
                return Ok(Some(decode_sgr_mouse(&csi, last)));
            }
            if last == b'u' {
                if first == b'?' {
                    return Ok(Some(decode_kitty_flags(&csi)));
                }
                return Ok(Some(decode_kitty(&csi)));
            }
            if csi.as_slice() == b"200~" {
                return read_bracketed_paste().map(Some);
            }
            if csi.as_slice() == b"I" {
                return Ok(Some(focus_event(true)));
            }
            if csi.as_slice() == b"O" {
                return Ok(Some(focus_event(false)));
            }
            if last == b'c' && (first == b'?' || first == b'>') {
                return Ok(Some(decode_device_attributes(&csi, first)));
            }
            // A `CSI…R` is a cursor-position report only when we solicited one;
            // otherwise it's modified-F3 (`CSI 1;<mod>R`) → fall through to keys.
            if last == b'R'
                && EXPECT_CPR.with(|c| {
                    let n = c.get();
                    if n > 0 {
                        c.set(n - 1);
                        true
                    } else {
                        false
                    }
                })
            {
                return Ok(Some(decode_cpr(&csi)));
            }
            if last == b'~' && csi.starts_with(b"27;") {
                return Ok(Some(decode_modify_other_keys(&csi)));
            }
            let (name, mbits) = parse_legacy_csi(&csi);
            let mut m = std::collections::BTreeMap::new();
            m.insert(Value::keyword("kind"), Value::keyword("key"));
            m.insert(Value::keyword("name"), Value::keyword(name));
            if let Some(mods) = mods_list(mbits) {
                m.insert(Value::keyword("mods"), mods);
            }
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

        // Alt + char (ESC followed by a character; may be multi-byte UTF-8).
        let alt_str = read_utf8_char(b2)?;
        let mut m = std::collections::BTreeMap::new();
        m.insert(Value::keyword("kind"), Value::keyword("alt"));
        m.insert(Value::keyword("char"), Value::string(&alt_str));
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
    let ch_str = read_utf8_char(b)?;
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
pub(crate) fn fs_offload<T: Send + 'static>(
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

/// Like [`fs_offload`], but `decode` may itself fail. Needed where the fetched
/// `T` requires further VM-thread-only processing that can reject — e.g.
/// `load`'s `sema_reader::read_many`, which interns symbols (not `Send`, so it
/// can't run on the worker) and can itself return a `SemaError` on a parse
/// error. `fs_offload`'s `decode: Fn(T) -> Value` has no way to signal that;
/// this sibling thread the `Result` through instead. Same drop-safety and
/// no-abort-hook rationale as `fs_offload` (see its doc comment) — a file read
/// finishes in bounded time, so cancellation is best-effort.
pub(crate) fn fs_offload_parse<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    decode: impl Fn(T) -> Result<Value, SemaError> + 'static,
) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use tokio::sync::oneshot::error::TryRecvError;

    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<T, String>>();
    sema_io::io_spawn_blocking(move || {
        let _ = tx.send(work());
        sema_core::notify_io_complete();
    });

    let handle = Rc::new(sema_core::IoHandle::new(move || match rx.try_recv() {
        Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
        Ok(Ok(t)) => match decode(t) {
            Ok(v) => sema_core::IoPoll::Ready(Ok(v)),
            Err(e) => sema_core::IoPoll::Ready(Err(e.to_string())),
        },
        Ok(Err(msg)) => sema_core::IoPoll::Ready(Err(msg)),
        Err(TryRecvError::Closed) => {
            sema_core::IoPoll::Ready(Err("file: worker dropped".to_string()))
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

// ── Async line-streaming (file/for-each-line, file/fold-lines[-bytes]) ────
//
// These natives run a Sema callback (`!Send`, touches `Value`s — cannot cross
// the thread boundary) once per line, so the whole read loop can't just be
// handed to `fs_offload` like a stateless read/write. Nor can the file be
// read to completion up front — an unbounded `fs_offload` slurp would defeat
// the "huge file" case these iterators exist for. Instead each offloaded
// round-trip reads a BOUNDED chunk of lines on the worker; the chunk is
// handed back and the callback runs per-line on the VM thread; then the next
// chunk is offloaded — repeating via a hand-rolled `IoHandle` poll closure
// (the same "poll immediately re-arms the next stage" shape as
// `stream.rs`'s `checkout_offload`, minus the checkout: the `BufReader` here
// is owned solely by this call, never shared, so there's nothing to
// reinstall/tombstone — an abandoned in-flight chunk read just finishes on
// the worker and is dropped, exactly like `fs_offload`'s own no-abort-hook
// rationale).

/// Bounded chunk size for the async line-streaming natives: read up to this
/// many lines OR this many bytes per offloaded round-trip — large enough to
/// amortize the worker hop, small enough that a huge file is never pulled
/// wholly into memory.
const ASYNC_LINE_CHUNK_MAX_LINES: usize = 256;
const ASYNC_LINE_CHUNK_MAX_BYTES: usize = 256 * 1024;

/// One bounded chunk read of newline-stripped `String` lines (the
/// `file/for-each-line` / `file/fold-lines` element type). Runs entirely on
/// the worker thread; `reader` and its `File` are `Send`.
fn read_line_chunk_str(
    reader: &mut std::io::BufReader<std::fs::File>,
    path: &str,
    op: &str,
) -> Result<(Vec<String>, bool), String> {
    let mut lines = Vec::new();
    let mut budget = 0usize;
    let mut eof = false;
    let mut line_buf = String::with_capacity(64);
    loop {
        line_buf.clear();
        let n = reader
            .read_line(&mut line_buf)
            .map_err(|e| fs_io_msg(format!("{op} {path}: {e}")))?;
        if n == 0 {
            eof = true;
            break;
        }
        if line_buf.ends_with('\n') {
            line_buf.pop();
            if line_buf.ends_with('\r') {
                line_buf.pop();
            }
        }
        budget += n;
        lines.push(std::mem::take(&mut line_buf));
        if lines.len() >= ASYNC_LINE_CHUNK_MAX_LINES || budget >= ASYNC_LINE_CHUNK_MAX_BYTES {
            break;
        }
    }
    Ok((lines, eof))
}

/// Byte-oriented sibling of [`read_line_chunk_str`] for `file/fold-lines-bytes`
/// — no UTF-8 validation, same `\r\n`/`\n` stripping rule.
fn read_line_chunk_bytes(
    reader: &mut std::io::BufReader<std::fs::File>,
    path: &str,
    op: &str,
) -> Result<(Vec<Vec<u8>>, bool), String> {
    let mut lines = Vec::new();
    let mut budget = 0usize;
    let mut eof = false;
    let mut line_buf: Vec<u8> = Vec::with_capacity(128);
    loop {
        line_buf.clear();
        let n = reader
            .read_until(b'\n', &mut line_buf)
            .map_err(|e| fs_io_msg(format!("{op} {path}: {e}")))?;
        if n == 0 {
            eof = true;
            break;
        }
        let mut end = line_buf.len();
        if end > 0 && line_buf[end - 1] == b'\n' {
            end -= 1;
            if end > 0 && line_buf[end - 1] == b'\r' {
                end -= 1;
            }
        }
        budget += n;
        lines.push(line_buf[..end].to_vec());
        if lines.len() >= ASYNC_LINE_CHUNK_MAX_LINES || budget >= ASYNC_LINE_CHUNK_MAX_BYTES {
            break;
        }
    }
    Ok((lines, eof))
}

/// A bounded chunk-read function for the async line streamers: reads at most
/// [`ASYNC_LINE_CHUNK_MAX_LINES`]/[`ASYNC_LINE_CHUNK_MAX_BYTES`] worth of `L`
/// items from `reader`, returning `(items, eof)`. Implemented by
/// [`read_line_chunk_str`] and [`read_line_chunk_bytes`]; always a plain `fn`
/// item (no captures), so it's trivially `Send` to move into the worker
/// closure alongside the reader.
type LineChunkReader<L> =
    fn(&mut std::io::BufReader<std::fs::File>, &str, &str) -> Result<(Vec<L>, bool), String>;

/// One offloaded chunk read's outcome: the reinstalled (still-open) reader,
/// the chunk of items read, and whether EOF was hit.
type LineChunkResult<L> = Result<(std::io::BufReader<std::fs::File>, Vec<L>, bool), String>;

/// Spawn one offloaded chunk read: opens `path` first if `reader` is `None`
/// (the first round-trip), otherwise resumes the given (still-open) reader.
/// Sends back the reinstalled reader alongside the chunk so the poller can
/// kick off the next round without reopening the file.
fn spawn_line_chunk<L: Send + 'static>(
    reader: Option<std::io::BufReader<std::fs::File>>,
    path: String,
    op: &'static str,
    read_chunk: LineChunkReader<L>,
) -> tokio::sync::oneshot::Receiver<LineChunkResult<L>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let result = (|| {
            let mut reader = match reader {
                Some(r) => r,
                None => {
                    let file = std::fs::File::open(&path)
                        .map_err(|e| fs_io_msg(format!("{op} {path}: {e}")))?;
                    std::io::BufReader::with_capacity(ASYNC_LINE_CHUNK_MAX_BYTES, file)
                }
            };
            let (items, eof) = read_chunk(&mut reader, &path, op)?;
            Ok((reader, items, eof))
        })();
        let _ = tx.send(result);
        sema_core::notify_io_complete();
    });
    rx
}

/// Drive the bounded-chunk read/callback loop described above to completion,
/// arming a fresh `AwaitIo` yield for each chunk in flight. `on_chunk` runs
/// ON THE VM THREAD once per chunk (it's where the Sema callback is invoked —
/// may hold `Value`s, e.g. a fold accumulator); `finish` builds the final
/// return value once EOF is reached. Returns `Ok(nil)` after arming the first
/// yield signal, like `fs_offload`.
fn async_stream_lines<L: Send + 'static>(
    op: &'static str,
    path: String,
    read_chunk: LineChunkReader<L>,
    mut on_chunk: impl FnMut(Vec<L>) -> Result<(), SemaError> + 'static,
    mut finish: impl FnMut() -> Value + 'static,
) -> Result<Value, SemaError> {
    use tokio::sync::oneshot::error::TryRecvError;

    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let rx0 = spawn_line_chunk(None, path.clone(), op, read_chunk);
    let rx = std::rc::Rc::new(std::cell::RefCell::new(rx0));

    let poll = move || -> sema_core::IoPoll {
        let recv = rx.borrow_mut().try_recv();
        match recv {
            Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
            Err(TryRecvError::Closed) => {
                sema_core::IoPoll::Ready(Err(format!("{op}: I/O worker dropped")))
            }
            Ok(Err(msg)) => sema_core::IoPoll::Ready(Err(msg)),
            Ok(Ok((reader, items, eof))) => {
                if let Err(e) = on_chunk(items) {
                    return sema_core::IoPoll::Ready(Err(e.to_string()));
                }
                if eof {
                    return sema_core::IoPoll::Ready(Ok(finish()));
                }
                *rx.borrow_mut() = spawn_line_chunk(Some(reader), path.clone(), op, read_chunk);
                sema_core::IoPoll::Pending
            }
        }
    };

    let handle = std::rc::Rc::new(sema_core::IoHandle::new(poll));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
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

/// Cooperatively poll `ready` on the scheduler thread until it yields a value or
/// `timeout_ms` milliseconds have elapsed since `started` (then `nil`), rather
/// than blocking the OS thread in a `select(2)`/`sleep` wait. The async-context
/// path of `io/read-key-timeout` and `event/select`: a "wait for input OR agent
/// progress" loop must not stall the single cooperative scheduler thread while
/// it waits, so we arm the same `AwaitIo` yield the file/http/shell async paths
/// use — its poll closure re-checks readiness each scheduler tick (every
/// sibling step, and at worst ~50 ms while every task is parked). `ready`
/// returns `Some(Ok(v))` when input is available, `Some(Err(e))` to reject the
/// task, or `None` while still waiting.
///
/// The timeout is checked as `started.elapsed() >= timeout_ms`, never as
/// `started + Duration` — `Instant + Duration` panics on overflow, and unlike
/// the sync path (`unix_stdin_ready`, a plain `libc::select` timeval) an
/// arbitrarily large `ms` must not be able to crash the scheduler thread.
///
/// Unlike the `fs_offload`/`shell_async` poll closures (which cross no thread
/// boundary but deliberately capture only `Send` data), `ready` here runs
/// entirely on the VM thread and MAY capture Sema `Value`s (e.g. `event/select`'s
/// source maps). That is sound: the cycle collector (Bacon–Rajan trial deletion)
/// cannot trace into the boxed closure, so any `Value` it holds keeps a strong
/// `Rc` the collector can't account for — but only for as long as the
/// `IoHandle` is alive. The handle (closure and all) is dropped the moment the
/// task resumes or is cancelled, which releases those `Rc`s normally; nothing
/// is pinned beyond the handle's own lifetime.
pub(crate) fn await_io_until(
    started: std::time::Instant,
    timeout_ms: u64,
    mut ready: impl FnMut() -> Option<Result<Value, String>> + 'static,
) -> Result<Value, SemaError> {
    let timeout = std::time::Duration::from_millis(timeout_ms);
    let handle = std::rc::Rc::new(sema_core::IoHandle::new(move || match ready() {
        Some(Ok(v)) => sema_core::IoPoll::Ready(Ok(v)),
        Some(Err(e)) => sema_core::IoPoll::Ready(Err(e)),
        None if started.elapsed() >= timeout => sema_core::IoPoll::Ready(Ok(Value::nil())),
        None => sema_core::IoPoll::Pending,
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
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
                .map(Value::string_owned)
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
                Value::string_owned,
            );
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| SemaError::Io(format!("file/read {path}: {e}")))?;
        Ok(Value::string_owned(content))
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
            if sema_core::in_async_context() {
                let path = path.to_string();
                let bv = bv.to_vec();
                return fs_offload(
                    move || {
                        std::fs::write(&path, &bv)
                            .map_err(|e| fs_io_msg(format!("file/write-bytes {path}: {e}")))
                    },
                    |()| Value::nil(),
                );
            }
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
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload(
                move || Ok(std::path::Path::new(&path).exists()),
                Value::bool,
            );
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
        Ok(Value::string_owned(input))
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

    // R7RS `raise`: raise an arbitrary object as an exception. Identical to the
    // `throw` special form, but a first-class procedure so it can be passed
    // around / partially applied. A raw object raises as a `UserException`,
    // recovered via the `{:type :user :value ...}` error map (`guard` unwraps
    // `:value` so its variable is the raw object); a caught condition map
    // re-raises as itself, so nested catch/re-raise guards stay idempotent.
    register_fn(env, "raise", |args| {
        check_arity!(args, "raise", 1);
        Err(SemaError::from_thrown(args[0].clone()))
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
            if sema_core::in_async_context() {
                let from = from.to_string();
                let to = to.to_string();
                return fs_offload(
                    move || {
                        std::fs::rename(&from, &to)
                            .map_err(|e| fs_io_msg(format!("file/rename {from} -> {to}: {e}")))
                    },
                    |()| Value::nil(),
                );
            }
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
        fn list_impl(path: &str) -> Result<Vec<String>, String> {
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(path).map_err(|e| format!("file/list {path}: {e}"))? {
                let entry = entry.map_err(|e| format!("file/list {path}: {e}"))?;
                entries.push(entry.file_name().to_string_lossy().into_owned());
            }
            Ok(entries)
        }
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload(
                move || list_impl(&path).map_err(fs_io_msg),
                |entries: Vec<String>| {
                    Value::list(entries.into_iter().map(|s| Value::string(&s)).collect())
                },
            );
        }
        let entries = list_impl(path).map_err(SemaError::Io)?;
        Ok(Value::list(
            entries.into_iter().map(|s| Value::string(&s)).collect(),
        ))
    });

    crate::register_fn_path_gated(env, sandbox, Caps::FS_WRITE, "file/mkdir", &[0], |args| {
        check_arity!(args, "file/mkdir", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload(
                move || {
                    std::fs::create_dir_all(&path)
                        .map_err(|e| fs_io_msg(format!("file/mkdir {path}: {e}")))
                },
                |()| Value::nil(),
            );
        }
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
            if sema_core::in_async_context() {
                let path = path.to_string();
                return fs_offload(
                    move || Ok(std::path::Path::new(&path).is_dir()),
                    Value::bool,
                );
            }
            Ok(Value::bool(std::path::Path::new(path).is_dir()))
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/is-file?", &[0], |args| {
        check_arity!(args, "file/is-file?", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload(
                move || Ok(std::path::Path::new(&path).is_file()),
                Value::bool,
            );
        }
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
            if sema_core::in_async_context() {
                let path = path.to_string();
                return fs_offload(
                    move || Ok(std::path::Path::new(&path).is_symlink()),
                    Value::bool,
                );
            }
            Ok(Value::bool(std::path::Path::new(path).is_symlink()))
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/info", &[0], |args| {
        check_arity!(args, "file/info", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // (size, is_dir, is_file, modified_millis) — plain Send data extracted from
        // `std::fs::Metadata` on the worker; the `Value` map is only built back on
        // the VM thread by `decode`/below.
        fn info_impl(path: &str) -> Result<(u64, bool, bool, Option<i64>), String> {
            let meta = std::fs::metadata(path).map_err(|e| format!("file/info {path}: {e}"))?;
            let modified = meta.modified().ok().and_then(|m| {
                m.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_millis() as i64)
            });
            Ok((meta.len(), meta.is_dir(), meta.is_file(), modified))
        }
        fn info_to_value(
            (size, is_dir, is_file, modified): (u64, bool, bool, Option<i64>),
        ) -> Value {
            let mut map = std::collections::BTreeMap::new();
            map.insert(Value::keyword("size"), Value::int(size as i64));
            map.insert(Value::keyword("is-dir"), Value::bool(is_dir));
            map.insert(Value::keyword("is-file"), Value::bool(is_file));
            if let Some(modified) = modified {
                map.insert(Value::keyword("modified"), Value::int(modified));
            }
            Value::map(map)
        }
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload(move || info_impl(&path).map_err(fs_io_msg), info_to_value);
        }
        let info = info_impl(path).map_err(SemaError::Io)?;
        Ok(info_to_value(info))
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
        if sema_core::in_async_context() {
            let s = s.to_string();
            return fs_offload(
                move || {
                    std::fs::canonicalize(&s)
                        .map(|p| p.to_string_lossy().into_owned())
                        .map_err(|e| fs_io_msg(format!("path/absolute {s}: {e}")))
                },
                Value::string_owned,
            );
        }
        let abs = std::fs::canonicalize(s)
            .map_err(|e| SemaError::Io(format!("path/absolute {s}: {e}")))?;
        Ok(Value::string(&abs.to_string_lossy()))
    });

    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "file/glob", |args| {
        check_arity!(args, "file/glob", 1);
        let pattern = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Validate the pattern up front (sandbox-side, cheap, and the error is a
        // `SemaError::eval` not `::Io` — keep it on the VM thread in both paths).
        glob::glob(pattern)
            .map_err(|e| SemaError::eval(format!("file/glob: invalid pattern: {e}")))?;
        fn glob_impl(pattern: &str) -> Result<Vec<String>, String> {
            let paths =
                glob::glob(pattern).map_err(|e| format!("file/glob: invalid pattern: {e}"))?;
            paths
                .collect::<Result<Vec<_>, _>>()
                .map(|entries| {
                    entries
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect()
                })
                .map_err(|e| format!("file/glob: {e}"))
        }
        if sema_core::in_async_context() {
            let pattern = pattern.to_string();
            return fs_offload(
                move || glob_impl(&pattern).map_err(fs_io_msg),
                |entries: Vec<String>| {
                    Value::list(entries.into_iter().map(|s| Value::string(&s)).collect())
                },
            );
        }
        let items = glob_impl(pattern)
            .map_err(SemaError::Io)?
            .into_iter()
            .map(|s| Value::string(&s))
            .collect();
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
            if sema_core::in_async_context() {
                let s = s.to_string();
                return fs_offload(
                    move || {
                        std::fs::canonicalize(&s)
                            .map(|p| p.to_string_lossy().into_owned())
                            .map_err(|e| fs_io_msg(format!("path/canonicalize {s}: {e}")))
                    },
                    Value::string_owned,
                );
            }
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
            if sema_core::in_async_context() {
                let path_s = path.to_string();
                let func_async = func.clone();
                // Snapshot the callback's captured upvalues NOW, while this
                // native's task VM is still current: the per-line callback runs
                // from a deferred I/O poll AFTER this task suspends, by which
                // point its stack is inactive and the upvalues would read nil.
                sema_vm::snapshot_escaping_closure(&func_async);
                return async_stream_lines::<String>(
                    "file/for-each-line",
                    path_s,
                    read_line_chunk_str,
                    move |lines| {
                        sema_core::with_stdlib_ctx(|ctx| {
                            for line in &lines {
                                // Run synchronously on a foreign VM (NOT the inline-task
                                // path): the scheduler is already taken driving this poll.
                                sema_vm::run_closure_foreign_sync(
                                    &func_async,
                                    ctx,
                                    &[Value::string(line)],
                                )?;
                            }
                            Ok(())
                        })
                    },
                    Value::nil,
                );
            }
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
            if sema_core::in_async_context() {
                let path_s = path.to_string();
                let func_async = func.clone();
                // See file/for-each-line: snapshot upvalues before yielding.
                sema_vm::snapshot_escaping_closure(&func_async);
                let acc_cell = std::rc::Rc::new(std::cell::RefCell::new(acc.clone()));
                let acc_for_finish = acc_cell.clone();
                return async_stream_lines::<String>(
                    "file/fold-lines",
                    path_s,
                    read_line_chunk_str,
                    move |lines| {
                        sema_core::with_stdlib_ctx(|ctx| {
                            for line in lines {
                                let acc = acc_cell.borrow().clone();
                                // Synchronous foreign-VM call (see for-each-line).
                                let new_acc = sema_vm::run_closure_foreign_sync(
                                    &func_async,
                                    ctx,
                                    &[acc, Value::string(&line)],
                                )?;
                                *acc_cell.borrow_mut() = new_acc;
                            }
                            Ok(())
                        })
                    },
                    move || acc_for_finish.borrow().clone(),
                );
            }
            let file = std::fs::File::open(path)
                .map_err(|e| SemaError::Io(format!("file/fold-lines {path}: {e}")))?;
            // 256KB buffer (vs default 8KB) improves throughput for large file reads.
            let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);

            sema_core::with_stdlib_ctx(|ctx| {
                let mut line_buf = String::with_capacity(64);
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
                    // Owned handoff: the accumulator is MOVED into the callback
                    // frame (no lingering caller ref), so together with the
                    // compiler's TakeLocal last-use moves a uniquely-owned map
                    // accumulator hits assoc's in-place fast path per line
                    // instead of deep-cloning.
                    let mut cb_args = [std::mem::replace(&mut acc, Value::nil()), line_val];
                    acc = sema_core::call_callback_owned(ctx, &func, &mut cb_args)?;
                }
                Ok(acc)
            })
        },
    );

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/fold-lines-bytes",
        &[0],
        |args| {
            check_arity!(args, "file/fold-lines-bytes", 3);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let func = args[1].clone();
            let mut acc = args[2].clone();
            if sema_core::in_async_context() {
                let path_s = path.to_string();
                let func_async = func.clone();
                // See file/for-each-line: snapshot upvalues before yielding.
                sema_vm::snapshot_escaping_closure(&func_async);
                let acc_cell = std::rc::Rc::new(std::cell::RefCell::new(acc.clone()));
                let acc_for_finish = acc_cell.clone();
                return async_stream_lines::<Vec<u8>>(
                    "file/fold-lines-bytes",
                    path_s,
                    read_line_chunk_bytes,
                    move |lines| {
                        sema_core::with_stdlib_ctx(|ctx| {
                            for line in lines {
                                let acc = acc_cell.borrow().clone();
                                // Synchronous foreign-VM call (see for-each-line).
                                let new_acc = sema_vm::run_closure_foreign_sync(
                                    &func_async,
                                    ctx,
                                    &[acc, Value::bytevector(line)],
                                )?;
                                *acc_cell.borrow_mut() = new_acc;
                            }
                            Ok(())
                        })
                    },
                    move || acc_for_finish.borrow().clone(),
                );
            }
            let file = std::fs::File::open(path)
                .map_err(|e| SemaError::Io(format!("file/fold-lines-bytes {path}: {e}")))?;
            // 256KB buffer (vs default 8KB) improves throughput for large file reads.
            let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);

            sema_core::with_stdlib_ctx(|ctx| {
                // One reusable read buffer per fold; each line is copied into
                // the bytevector the callback receives (the callback may
                // retain it, so the buffer itself cannot be handed out). No
                // UTF-8 validation — this is the byte-oriented sibling of
                // file/fold-lines for `bytes/*` pipelines.
                let mut line_buf: Vec<u8> = Vec::with_capacity(128);
                loop {
                    line_buf.clear();
                    let n = reader
                        .read_until(b'\n', &mut line_buf)
                        .map_err(|e| SemaError::Io(format!("file/fold-lines-bytes {path}: {e}")))?;
                    if n == 0 {
                        break;
                    }
                    // Lines carry no terminator: strip a trailing \n or \r\n.
                    // The \r is only stripped as part of a \r\n pair — a bare
                    // \r at EOF is line content, exactly as in file/fold-lines.
                    let mut end = line_buf.len();
                    if end > 0 && line_buf[end - 1] == b'\n' {
                        end -= 1;
                        if end > 0 && line_buf[end - 1] == b'\r' {
                            end -= 1;
                        }
                    }
                    let line_val = Value::bytevector(line_buf[..end].to_vec());
                    // Owned handoff — see file/fold-lines: the accumulator is
                    // moved into the callback frame so uniqueness-gated
                    // in-place fast paths can fire inside the callback.
                    let mut cb_args = [std::mem::replace(&mut acc, Value::nil()), line_val];
                    acc = sema_core::call_callback_owned(ctx, &func, &mut cb_args)?;
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
            if sema_core::in_async_context() {
                let path = path.to_string();
                return fs_offload(
                    move || {
                        std::fs::write(&path, &content)
                            .map_err(|e| fs_io_msg(format!("file/write-lines {path}: {e}")))
                    },
                    |()| Value::nil(),
                );
            }
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
        Ok(Value::string_owned(buf))
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

            // In an async task, the raw `libc::read(STDIN)` below blocks the
            // single cooperative VM thread with no timeout — worse than
            // `io/read-key-timeout`, which at least bounds the stall. Reuse the
            // same cooperative park (`await_io_until` + `unix_stdin_ready(0)`
            // polling), just with an effectively unbounded timeout so it waits
            // for a key exactly like the sync path, without blocking siblings.
            // The sync path below is byte-identical to before.
            if sema_core::in_async_context() {
                if let Some(v) = sema_core::take_resume_value() {
                    return Ok(v);
                }
                let started = std::time::Instant::now();
                return await_io_until(started, u64::MAX, || {
                    if !unix_stdin_ready(0) {
                        return None;
                    }
                    match parse_key_input() {
                        Ok(Some(v)) => Some(Ok(v)),
                        Ok(None) => {
                            STDIN_EOF.with(|f| f.set(true));
                            Some(Ok(Value::nil()))
                        }
                        Err(e) => Some(Err(e.to_string())),
                    }
                });
            }

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

            // In an async task, park on a cooperative poll instead of blocking the
            // scheduler thread in `select(2)` for the whole timeout, so a "key OR
            // agent progress" loop lets sibling tasks run while it waits. Once a
            // first byte is ready `parse_key_input` may briefly block reading a
            // multi-byte sequence — identical to the sync path — but the idle wait
            // no longer does. The sync path below is byte-identical to before.
            if sema_core::in_async_context() {
                if let Some(v) = sema_core::take_resume_value() {
                    return Ok(v);
                }
                let started = std::time::Instant::now();
                return await_io_until(started, ms, || {
                    if !unix_stdin_ready(0) {
                        return None;
                    }
                    match parse_key_input() {
                        Ok(Some(v)) => Some(Ok(v)),
                        Ok(None) => {
                            STDIN_EOF.with(|f| f.set(true));
                            Some(Ok(Value::nil()))
                        }
                        Err(e) => Some(Err(e.to_string())),
                    }
                });
            }

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

        // Capability probes (raw mode required; they round-trip a query + reply).
        // term/supports-kitty-keys? → bool via `CSI ?u` + DSR barrier.
        register_fn(env, "term/supports-kitty-keys?", |args| {
            check_arity!(args, "term/supports-kitty-keys?", 0);
            Ok(Value::bool(probe_kitty_support()?))
        });
        // term/cursor-position → {:row :col} (or nil) via a DSR round-trip.
        register_fn(env, "term/cursor-position", |args| {
            check_arity!(args, "term/cursor-position", 0);
            query_cursor_position()
        });
        // term/query-cursor-position → write DSR and arm the CPR flag, so the
        // reply (arriving later via io/read-key) is decoded as :cpr rather than
        // being mistaken for modified-F3 (`CSI 1;<mod>R`).
        register_fn(env, "term/query-cursor-position", |args| {
            check_arity!(args, "term/query-cursor-position", 0);
            EXPECT_CPR.with(|c| c.set(c.get().saturating_add(1)));
            write_stdout("\x1b[6n")?;
            Ok(Value::nil())
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
        if sema_core::in_async_context() {
            let path = path.to_string();
            return fs_offload_parse(
                move || {
                    std::fs::read_to_string(&path)
                        .map_err(|e| fs_io_msg(format!("load {path}: {e}")))
                },
                |content| {
                    // Parsing/interning isn't `Send` — runs back on the VM thread.
                    let exprs = sema_reader::read_many(&content)?;
                    Ok(Value::list(exprs))
                },
            );
        }
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

/// Async-context coverage for the `file/*`/`path/*`/`load`/`io/read-key`
/// scheduler-offload gates added to this file. `sema-stdlib` doesn't depend
/// on `sema-vm`/`sema-eval` (the real scheduler + interpreter live there), so
/// these tests stand in for the scheduler by hand: force
/// `sema_core::in_async_context()` on, call the native, then poll the
/// `AwaitIo` handle it arms to completion — exactly what the scheduler does
/// in production, just single-threaded and synchronous here.
#[cfg(test)]
mod async_offload_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    /// Forces `in_async_context()` on for the guard's lifetime, resetting it
    /// (even on panic/early return) so a failure can't leak the flag into
    /// whichever test the harness runs next on the same worker thread —
    /// mirrors `server.rs`'s `ResetAsyncContext` test guard.
    struct AsyncCtxGuard;
    impl Drop for AsyncCtxGuard {
        fn drop(&mut self) {
            sema_core::set_async_context(false);
        }
    }

    /// A minimal `CallCallbackFn`: invokes a `Value` that must be a native
    /// fn directly. Stands in for the full interpreter's call dispatch
    /// (`sema-vm`/`sema-eval`) so the async path of `file/for-each-line` /
    /// `file/fold-lines*` — which threads the Sema callback through
    /// `sema_core::call_callback` on the VM thread — has something to call
    /// in a unit test that has no interpreter.
    fn invoke_native_value(
        ctx: &sema_core::EvalContext,
        func: &Value,
        args: &[Value],
    ) -> Result<Value, SemaError> {
        let nf = func
            .as_native_fn_ref()
            .ok_or_else(|| SemaError::eval("test harness: callback must be a native fn"))?;
        (nf.func)(ctx, args)
    }

    fn make_env() -> sema_core::Env {
        let env = sema_core::Env::new();
        register(&env, &sema_core::Sandbox::allow_all());
        env
    }

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sema-io-async-test-{tag}-{nanos}"))
    }

    /// Look up a registered native by name, returning a callable closure that
    /// invokes it with a fresh `EvalContext` each time (mirroring how the VM
    /// calls a native).
    fn native(env: &sema_core::Env, name: &str) -> impl Fn(&[Value]) -> Result<Value, SemaError> {
        let f = env
            .get(sema_core::intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        move |args: &[Value]| {
            let nf = f.as_native_fn_ref().expect("native fn");
            let ctx = sema_core::EvalContext::new();
            (nf.func)(&ctx, args)
        }
    }

    fn call_sync(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        native(env, name)(args).expect("sync call ok")
    }

    /// Call a native fn with the async-context gate forced on, then drive the
    /// `AwaitIo` handle it arms to completion by polling. Panics if the
    /// native didn't yield at all (e.g. it silently took the sync fallback)
    /// or the offload rejects.
    fn drive_async(call: impl FnOnce() -> Result<Value, SemaError>) -> Value {
        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let armed = call().expect("native call should arm a yield, not error synchronously");
        assert_eq!(
            armed,
            Value::nil(),
            "an offloading native returns nil immediately after arming its yield signal"
        );
        let reason = sema_core::take_yield_signal()
            .expect("expected a yield signal to be armed — did the native take the sync path?");
        let handle = match reason {
            sema_core::YieldReason::AwaitIo(h) => h,
            other => panic!("expected an AwaitIo yield, got {other:?}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match handle.poll() {
                sema_core::IoPoll::Ready(Ok(v)) => return v,
                sema_core::IoPoll::Ready(Err(e)) => panic!("offload rejected: {e}"),
                sema_core::IoPoll::Pending => {
                    assert!(
                        Instant::now() < deadline,
                        "offload never completed within 10s"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    // ── Stateless fs_offload gates ─────────────────────────────────────

    #[test]
    fn file_mkdir_offloads_and_creates_dir_async() {
        let env = make_env();
        let dir = tmp_path("mkdir");
        let dir_s = dir.to_string_lossy().to_string();
        let result = drive_async(|| native(&env, "file/mkdir")(&[Value::string(&dir_s)]));
        assert_eq!(result, Value::nil());
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Same native, sync path — confirms the added async gate left the
    /// default (non-async) behavior byte-for-byte unchanged.
    #[test]
    fn file_mkdir_sync_path_unchanged() {
        let env = make_env();
        let dir = tmp_path("mkdir-sync");
        let dir_s = dir.to_string_lossy().to_string();
        let result = call_sync(&env, "file/mkdir", &[Value::string(&dir_s)]);
        assert_eq!(result, Value::nil());
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_write_bytes_offloads_and_writes_async() {
        let env = make_env();
        let path = tmp_path("write-bytes.bin");
        let path_s = path.to_string_lossy().to_string();
        let content = b"hello async bytes".to_vec();
        let result = drive_async(|| {
            native(&env, "file/write-bytes")(&[
                Value::string(&path_s),
                Value::bytevector(content.clone()),
            ])
        });
        assert_eq!(result, Value::nil());
        assert_eq!(std::fs::read(&path).unwrap(), content);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_rename_offloads_async() {
        let env = make_env();
        let from = tmp_path("rename-from.txt");
        let to = tmp_path("rename-to.txt");
        std::fs::write(&from, "payload").unwrap();
        let (from_s, to_s) = (
            from.to_string_lossy().to_string(),
            to.to_string_lossy().to_string(),
        );
        let result = drive_async(|| {
            native(&env, "file/rename")(&[Value::string(&from_s), Value::string(&to_s)])
        });
        assert_eq!(result, Value::nil());
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "payload");
        let _ = std::fs::remove_file(&to);
    }

    #[test]
    fn file_list_offloads_async() {
        let env = make_env();
        let dir = tmp_path("list");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        let dir_s = dir.to_string_lossy().to_string();
        let result = drive_async(|| native(&env, "file/list")(&[Value::string(&dir_s)]));
        let mut names: Vec<String> = result
            .as_list()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.txt".to_string(), "b.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_glob_offloads_async() {
        let env = make_env();
        let dir = tmp_path("glob");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.log"), "b").unwrap();
        let pattern = format!("{}/*.txt", dir.to_string_lossy());
        let result = drive_async(|| native(&env, "file/glob")(&[Value::string(&pattern)]));
        let list = result.as_list().unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].as_str().unwrap().ends_with("a.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_canonicalize_offloads_async_matches_sync() {
        let env = make_env();
        let dir = tmp_path("canon");
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().to_string();
        let async_result =
            drive_async(|| native(&env, "path/canonicalize")(&[Value::string(&dir_s)]));
        let sync_result = call_sync(&env, "path/canonicalize", &[Value::string(&dir_s)]);
        assert_eq!(async_result, sync_result);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_absolute_offloads_async_matches_sync() {
        let env = make_env();
        let dir = tmp_path("abs");
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().to_string();
        let async_result = drive_async(|| native(&env, "path/absolute")(&[Value::string(&dir_s)]));
        let sync_result = call_sync(&env, "path/absolute", &[Value::string(&dir_s)]);
        assert_eq!(async_result, sync_result);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stat_predicates_offload_async() {
        let env = make_env();
        let dir = tmp_path("stat");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, "content").unwrap();
        let dir_s = dir.to_string_lossy().to_string();
        let file_s = file.to_string_lossy().to_string();

        assert_eq!(
            drive_async(|| native(&env, "file/is-directory?")(&[Value::string(&dir_s)])),
            Value::bool(true)
        );
        assert_eq!(
            drive_async(|| native(&env, "file/is-file?")(&[Value::string(&file_s)])),
            Value::bool(true)
        );
        assert_eq!(
            drive_async(|| native(&env, "file/is-symlink?")(&[Value::string(&file_s)])),
            Value::bool(false)
        );
        assert_eq!(
            drive_async(|| native(&env, "file/exists?")(&[Value::string(&file_s)])),
            Value::bool(true)
        );
        let info = drive_async(|| native(&env, "file/info")(&[Value::string(&file_s)]));
        let bt = info.as_map_ref().expect("map");
        assert_eq!(
            bt.get(&Value::keyword("size")).cloned(),
            Some(Value::int(7))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_write_lines_offloads_async() {
        let env = make_env();
        let path = tmp_path("write-lines.txt");
        let path_s = path.to_string_lossy().to_string();
        let lines = Value::list(vec![Value::string("a"), Value::string("b")]);
        let result = drive_async(|| {
            native(&env, "file/write-lines")(&[Value::string(&path_s), lines.clone()])
        });
        assert_eq!(result, Value::nil());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_offloads_read_and_parses_on_vm_thread_async() {
        let env = make_env();
        let path = tmp_path("load.sema");
        std::fs::write(&path, "(+ 1 2) (list 3 4)").unwrap();
        let path_s = path.to_string_lossy().to_string();
        let result = drive_async(|| native(&env, "load")(&[Value::string(&path_s)]));
        let exprs = result.as_list().expect("list of parsed exprs");
        assert_eq!(exprs.len(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_offload_rejects_missing_file_like_sync() {
        let env = make_env();
        let path = tmp_path("does-not-exist.sema");
        let path_s = path.to_string_lossy().to_string();
        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let armed = native(&env, "load")(&[Value::string(&path_s)]).expect("arms a yield");
        assert_eq!(armed, Value::nil());
        let reason = sema_core::take_yield_signal().expect("yield armed");
        let handle = match reason {
            sema_core::YieldReason::AwaitIo(h) => h,
            other => panic!("expected AwaitIo, got {other:?}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match handle.poll() {
                sema_core::IoPoll::Ready(Err(msg)) => {
                    assert!(msg.contains("load"), "error should name the op: {msg}");
                    break;
                }
                sema_core::IoPoll::Ready(Ok(v)) => panic!("expected a rejection, got {v:?}"),
                sema_core::IoPoll::Pending => {
                    assert!(Instant::now() < deadline, "never completed within 10s");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    // ── io/read-key: must yield, never block, in async context ─────────

    /// No real/interactive stdin is available in a test process, so this only
    /// asserts the property the offload exists to guarantee: the native
    /// returns immediately with a yield ARMED (an `AwaitIo` the scheduler can
    /// poll) instead of blocking the calling thread in the raw
    /// `libc::read(STDIN)` forever.
    #[cfg(unix)]
    #[test]
    fn io_read_key_arms_await_io_instead_of_blocking_in_async_context() {
        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let env = make_env();
        let result =
            native(&env, "io/read-key")(&[]).expect("should return immediately (armed yield)");
        assert_eq!(result, Value::nil());
        let reason = sema_core::take_yield_signal().expect("expected an AwaitIo yield to be armed");
        assert!(matches!(reason, sema_core::YieldReason::AwaitIo(_)));
    }

    // ── Line-streaming natives (chunked offload + VM-thread callback) ──

    #[test]
    fn read_line_chunk_str_strips_crlf_and_respects_line_budget() {
        let path = tmp_path("chunk-str.txt");
        let mut content = String::new();
        for i in 0..(ASYNC_LINE_CHUNK_MAX_LINES + 5) {
            content.push_str(&format!("l{i}\r\n"));
        }
        std::fs::write(&path, &content).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let mut reader = std::io::BufReader::new(file);

        let (chunk1, eof1) = read_line_chunk_str(&mut reader, "path", "test").unwrap();
        assert_eq!(chunk1.len(), ASYNC_LINE_CHUNK_MAX_LINES);
        assert!(!eof1, "budget-bounded chunk must not report eof early");
        assert_eq!(chunk1[0], "l0");
        assert!(!chunk1[0].contains('\r') && !chunk1[0].contains('\n'));

        let (chunk2, eof2) = read_line_chunk_str(&mut reader, "path", "test").unwrap();
        assert_eq!(chunk2.len(), 5);
        assert!(eof2);
        assert_eq!(chunk2[4], format!("l{}", ASYNC_LINE_CHUNK_MAX_LINES + 4));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_line_chunk_bytes_strips_crlf_no_utf8_check() {
        let path = tmp_path("chunk-bytes.bin");
        let mut content: Vec<u8> = Vec::new();
        content.extend_from_slice(b"a\r\n");
        content.extend_from_slice(&[0xff, 0xfe, b'\n']); // invalid UTF-8, must survive untouched
        content.extend_from_slice(b"tail-no-newline");
        std::fs::write(&path, &content).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let mut reader = std::io::BufReader::new(file);

        let (chunk, eof) = read_line_chunk_bytes(&mut reader, "path", "test").unwrap();
        assert_eq!(chunk.len(), 3);
        assert_eq!(chunk[0], b"a".to_vec());
        assert_eq!(chunk[1], vec![0xff, 0xfe]);
        assert_eq!(chunk[2], b"tail-no-newline".to_vec());
        assert!(eof);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_for_each_line_streams_multiple_chunks_async() {
        let env = make_env();
        let cb_ctx = sema_core::EvalContext::new();
        sema_core::set_call_callback(&cb_ctx, invoke_native_value);

        let path = tmp_path("for-each-line.txt");
        // More lines than one chunk holds, so the offload must round-trip
        // more than once through the worker.
        let n = ASYNC_LINE_CHUNK_MAX_LINES * 2 + 3;
        let content: String = (0..n).map(|i| format!("line-{i}\n")).collect();
        std::fs::write(&path, &content).unwrap();
        let path_s = path.to_string_lossy().to_string();

        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_for_cb = seen.clone();
        let collector =
            Value::native_fn(sema_core::NativeFn::simple("test-collect", move |args| {
                if let Some(s) = args.first().and_then(|v| v.as_str()) {
                    seen_for_cb.borrow_mut().push(s.to_string());
                }
                Ok(Value::nil())
            }));

        let result = drive_async(|| {
            native(&env, "file/for-each-line")(&[Value::string(&path_s), collector.clone()])
        });
        assert_eq!(result, Value::nil());
        let seen = seen.borrow();
        assert_eq!(seen.len(), n);
        assert_eq!(seen[0], "line-0");
        assert_eq!(seen[n - 1], format!("line-{}", n - 1));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_fold_lines_accumulates_across_chunks_async() {
        let env = make_env();
        let cb_ctx = sema_core::EvalContext::new();
        sema_core::set_call_callback(&cb_ctx, invoke_native_value);

        let path = tmp_path("fold-lines.txt");
        let n = ASYNC_LINE_CHUNK_MAX_LINES + 10;
        let content: String = (0..n).map(|_| "x\n".to_string()).collect();
        std::fs::write(&path, &content).unwrap();
        let path_s = path.to_string_lossy().to_string();

        let counter = Value::native_fn(sema_core::NativeFn::simple("test-count", |args| {
            let acc = args[0].as_int().unwrap_or(0);
            Ok(Value::int(acc + 1))
        }));

        let result = drive_async(|| {
            native(&env, "file/fold-lines")(&[
                Value::string(&path_s),
                counter.clone(),
                Value::int(0),
            ])
        });
        assert_eq!(result.as_int(), Some(n as i64));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_fold_lines_bytes_accumulates_across_chunks_async() {
        let env = make_env();
        let cb_ctx = sema_core::EvalContext::new();
        sema_core::set_call_callback(&cb_ctx, invoke_native_value);

        let path = tmp_path("fold-lines-bytes.bin");
        let n = ASYNC_LINE_CHUNK_MAX_LINES + 7;
        let mut content = Vec::new();
        for _ in 0..n {
            content.extend_from_slice(b"y\n");
        }
        std::fs::write(&path, &content).unwrap();
        let path_s = path.to_string_lossy().to_string();

        let counter = Value::native_fn(sema_core::NativeFn::simple("test-count-bytes", |args| {
            let acc = args[0].as_int().unwrap_or(0);
            Ok(Value::int(acc + 1))
        }));

        let result = drive_async(|| {
            native(&env, "file/fold-lines-bytes")(&[
                Value::string(&path_s),
                counter.clone(),
                Value::int(0),
            ])
        });
        assert_eq!(result.as_int(), Some(n as i64));

        let _ = std::fs::remove_file(&path);
    }
}
