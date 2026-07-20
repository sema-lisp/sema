use std::cell::{Cell, RefCell};
use std::io::BufRead;
#[cfg(target_arch = "wasm32")]
use std::io::Read as _;
use std::io::Write as _;

use sema_core::{check_arity, Caps, NativeFn, SemaError, Value, ValueView};

use crate::register_fn;

// Thread-local EOF flag: set when any stdin read returns 0 bytes (EOF)
thread_local! {
    static STDIN_EOF: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn mark_stdin_eof() {
    STDIN_EOF.with(|flag| flag.set(true));
}

#[cfg(not(target_arch = "wasm32"))]
fn read_line_value(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "read-line", 0);
    match crate::stream::stdin_text_line_value("read-line")? {
        Some(line) => Ok(Value::string_owned(line)),
        None => {
            mark_stdin_eof();
            Ok(Value::nil())
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn read_line_value(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "read-line", 0);
    let mut input = String::new();
    let read = std::io::stdin()
        .read_line(&mut input)
        .map_err(|error| SemaError::Io(format!("read-line: {error}")))?;
    if read == 0 {
        mark_stdin_eof();
        return Ok(Value::nil());
    }
    if input.ends_with('\n') {
        input.pop();
        if input.ends_with('\r') {
            input.pop();
        }
    }
    Ok(Value::string_owned(input))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_stdin_value(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "read-stdin", 0);
    let input = crate::stream::stdin_text_value("read-stdin")?;
    mark_stdin_eof();
    Ok(Value::string_owned(input))
}

#[cfg(target_arch = "wasm32")]
fn read_stdin_value(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "read-stdin", 0);
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| SemaError::Io(format!("read-stdin: {error}")))?;
    mark_stdin_eof();
    Ok(Value::string_owned(input))
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

    #[test]
    fn runtime_key_decoder_preserves_character_csi_and_paste_shapes() {
        let character = decode_runtime_key("ø".as_bytes());
        assert!(is_kw(&character, "kind", "char"));
        assert_eq!(kw(&character, "char"), Some(Value::string("ø")));

        let right = decode_runtime_key(b"\x1b[1;5C");
        assert!(is_kw(&right, "kind", "key"));
        assert!(is_kw(&right, "name", "right"));
        assert_eq!(mods_of(&right), vec!["ctrl"]);

        let paste = decode_runtime_key(b"\x1b[200~hello\x1b[201~");
        assert!(is_kw(&paste, "kind", "paste"));
        assert_eq!(kw(&paste, "text"), Some(Value::string("hello")));
    }

    #[test]
    fn runtime_key_framing_waits_for_complete_escape_sequences() {
        assert!(!runtime_key_complete(b"\x1b[", None));
        assert!(!runtime_key_complete(b"\x1b[1;5", None));
        assert!(runtime_key_complete(b"\x1b[1;5C", None));
        assert!(!runtime_key_complete(b"\x1b[200~hello", None));
        assert!(runtime_key_complete(b"\x1b[200~hello\x1b[201~", None));
    }

    #[test]
    fn kitty_query_timeout_preserves_boolean_contract() {
        let outcome =
            terminal_query_runtime_with_timeout(TerminalQueryKind::KittySupport, Duration::ZERO)
                .expect("kitty query timeout returns its fallback");
        let NativeOutcome::Return(value) = outcome else {
            panic!("zero-timeout kitty query must return immediately");
        };
        assert_eq!(value, Value::bool(false));
    }
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
    let value =
        run_terminal_query_value(TerminalQueryKind::KittySupport, Duration::from_millis(200))?;
    Ok(value.as_bool().unwrap_or(false))
}

/// Round-trip the cursor position: send DSR (`CSI 6n`) and return `{:row :col}`
/// from the reply, or nil (not a TTY / no reply). Must be called in raw mode.
#[cfg(unix)]
fn query_cursor_position() -> Result<Value, SemaError> {
    if !stdin_is_tty() {
        return Ok(Value::nil());
    }
    write_stdout("\x1b[6n")?;
    run_terminal_query_value(
        TerminalQueryKind::CursorPosition,
        Duration::from_millis(200),
    )
}

#[cfg(unix)]
fn key_character(kind: &str, character: String) -> Value {
    let mut map = std::collections::BTreeMap::new();
    map.insert(Value::keyword("kind"), Value::keyword(kind));
    map.insert(Value::keyword("char"), Value::string_owned(character));
    Value::map(map)
}

#[cfg(unix)]
fn named_key(name: &str) -> Value {
    let mut map = std::collections::BTreeMap::new();
    map.insert(Value::keyword("kind"), Value::keyword("key"));
    map.insert(Value::keyword("name"), Value::keyword(name));
    Value::map(map)
}

/// Decode one complete key event collected by the runtime's nonblocking poll.
/// An incomplete terminal sequence remains buffered between VM quanta instead
/// of blocking the VM thread for its next byte.
#[cfg(unix)]
fn decode_runtime_key(bytes: &[u8]) -> Value {
    let first = bytes.first().copied().unwrap_or_default();
    if first == 0x1b {
        let Some(second) = bytes.get(1).copied() else {
            return named_key("esc");
        };
        if second == b'[' {
            let csi = &bytes[2..];
            let last = csi.last().copied().unwrap_or_default();
            let first = csi.first().copied().unwrap_or_default();
            if csi.starts_with(b"200~") {
                const PREFIX_LEN: usize = 4;
                const TERMINATOR: &[u8] = b"\x1b[201~";
                let payload_end = csi.len().saturating_sub(TERMINATOR.len());
                let payload = &csi[PREFIX_LEN.min(payload_end)..payload_end];
                let mut map = std::collections::BTreeMap::new();
                map.insert(Value::keyword("kind"), Value::keyword("paste"));
                map.insert(
                    Value::keyword("text"),
                    Value::string_owned(String::from_utf8_lossy(payload).into_owned()),
                );
                return Value::map(map);
            }
            if first == b'<' {
                return decode_sgr_mouse(csi, last);
            }
            if last == b'u' {
                return if first == b'?' {
                    decode_kitty_flags(csi)
                } else {
                    decode_kitty(csi)
                };
            }
            if csi == b"I" {
                return focus_event(true);
            }
            if csi == b"O" {
                return focus_event(false);
            }
            if last == b'c' && (first == b'?' || first == b'>') {
                return decode_device_attributes(csi, first);
            }
            if last == b'R'
                && EXPECT_CPR.with(|counter| {
                    let pending = counter.get();
                    if pending == 0 {
                        false
                    } else {
                        counter.set(pending - 1);
                        true
                    }
                })
            {
                return decode_cpr(csi);
            }
            if last == b'~' && csi.starts_with(b"27;") {
                return decode_modify_other_keys(csi);
            }
            let (name, modifier_bits) = parse_legacy_csi(csi);
            let mut map = std::collections::BTreeMap::new();
            map.insert(Value::keyword("kind"), Value::keyword("key"));
            map.insert(Value::keyword("name"), Value::keyword(name));
            if let Some(modifiers) = mods_list(modifier_bits) {
                map.insert(Value::keyword("mods"), modifiers);
            }
            return Value::map(map);
        }
        if second == b'O' {
            let name = match bytes.get(2).copied().unwrap_or_default() {
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
            return named_key(name);
        }
        let character = std::str::from_utf8(&bytes[1..]).unwrap_or("?").to_string();
        return key_character("alt", character);
    }

    match first {
        0x7f | 0x08 => named_key("backspace"),
        0x09 => named_key("tab"),
        0x0a | 0x0d => named_key("enter"),
        byte if byte < 0x20 => key_character("ctrl", char::from(byte + 0x60).to_string()),
        byte if byte < 0x80 => key_character("char", char::from(byte).to_string()),
        _ => key_character(
            "char",
            std::str::from_utf8(bytes).unwrap_or("?").to_string(),
        ),
    }
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

// ─── Canonical quarantined-bounded external file operations (Task 05 R08A) ───
//
// Under the unified runtime (`in_runtime_quantum()`) the finite file ops route
// through the CANONICAL external-wait path: a `PreparedExternalOperation`
// submitted to the real thread-pool executor, exactly like `sleep`/`mcp/call`.
// Each is classified `QuarantinedBounded` — a hard byte/entry cap is fixed on
// the VM thread BEFORE dispatch, the job carries only an owned `Send` input
// snapshot (a `String`/`Vec<u8>`, never an `Rc`/`Value`), computes off-thread,
// and its send-safe payload is decoded back into a `Value` on the VM thread.
// Every runtime caller receives a structural External suspension; synchronous
// callers use the native's value ABI.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    downcast_send_payload, CompletionDecoder, CompletionKind, DecodedCompletion, ExternalFailure,
    NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, NativeSuspend,
    PreparedExternalOperation, QuarantineBound, ResumeInput, SendPayload, Trace, WaitKind,
};

/// Completion tag for a quarantined file operation. A tag only needs to be
/// consistent between the issued identity and the prepared op; it is not a
/// uniqueness key, so one shared value for all file ops is correct.
const FS_COMPLETION_KIND: u64 = 1;

/// Default hard byte cap for a single finite file read/write routed to a worker.
/// Fixed BEFORE dispatch (via `stat`/in-memory length), it bounds the worst-case
/// allocation the quarantined job can produce.
pub const FS_BYTE_CAP_DEFAULT: u64 = 256 * 1024 * 1024; // 256 MiB
/// Default hard entry cap for a single finite `file/list`. The cap is fixed
/// before dispatch and stored in the job; the worker aborts if it discovers more.
pub const FS_LIST_CAP_DEFAULT: u64 = 5_000_000;

static FS_BYTE_CAP: AtomicU64 = AtomicU64::new(FS_BYTE_CAP_DEFAULT);
static FS_LIST_CAP: AtomicU64 = AtomicU64::new(FS_LIST_CAP_DEFAULT);

/// Live count of quarantined file jobs executing on a worker, and the peak that
/// count has reached. The peak proves genuine off-thread OVERLAP (two spawned
/// file ops in flight at once → peak >= 2) without depending on wall-clock
/// timing. Process-global; a test resets it before observing.
static FS_INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static FS_PEAK_INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Test-only artificial delay (ms) held inside every quarantined file job while
/// it occupies its in-flight slot. Defaults to 0 (a single relaxed atomic load
/// in production, no behavior change); a test raises it to make two concurrent
/// jobs demonstrably overlap or to keep a job parked long enough to cancel it.
static FS_TEST_DELAY_MS: AtomicU64 = AtomicU64::new(0);

/// A cancelled quarantined file job is detached and reaped when its (now unowned)
/// completion lands. This deadline is the CleanupRegistry watchdog: a bounded
/// file job completes in well under this, so its cleanup entry is always reaped
/// in time; a wedged worker past it faults the runtime rather than leaking.
const FS_CLEANUP_DEADLINE: Duration = Duration::from_secs(120);

/// Peak simultaneous in-flight quarantined file jobs since the last reset.
pub fn fs_peak_inflight() -> usize {
    FS_PEAK_INFLIGHT.load(AtomicOrdering::SeqCst)
}

/// Quarantined file jobs currently executing on a worker.
pub fn fs_current_inflight() -> usize {
    FS_INFLIGHT.load(AtomicOrdering::SeqCst)
}

/// Reset the in-flight peak gauge (call before observing overlap in a test).
pub fn reset_fs_inflight() {
    FS_PEAK_INFLIGHT.store(0, AtomicOrdering::SeqCst);
}

/// Override the finite-file byte cap (test hook for the pre-dispatch cap gate).
pub fn set_fs_byte_cap(bytes: u64) {
    FS_BYTE_CAP.store(bytes.max(1), AtomicOrdering::SeqCst);
}

/// Override the `file/list` entry cap (test hook for the pre-dispatch cap gate).
pub fn set_fs_list_cap(entries: u64) {
    FS_LIST_CAP.store(entries.max(1), AtomicOrdering::SeqCst);
}

/// Set the test-only per-job delay (ms). Pass 0 to disable.
pub fn set_fs_test_delay_ms(ms: u64) {
    FS_TEST_DELAY_MS.store(ms, AtomicOrdering::SeqCst);
}

/// Decodes a quarantined file job's send-safe payload back into a `Value` on the
/// VM thread. The payload is a `Result<T, String>`: `Ok(T)` maps through
/// `to_value`; `Err(message)` is a domain I/O error rendered exactly like the
/// synchronous path (`SemaError::Io(message)`). A worker-level `ExternalFailure`
/// (panic / cancellation / bound-exceeded) surfaces as an evaluation error.
struct FsDecoder<T: Send + 'static> {
    op: &'static str,
    to_value: fn(T) -> Value,
}

impl<T: Send + 'static> Trace for FsDecoder<T> {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl<T: Send + 'static> CompletionDecoder for FsDecoder<T> {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        match result {
            Ok(payload) => match downcast_send_payload::<Result<T, String>>(payload, self.op) {
                Ok(Ok(value)) => Ok((self.to_value)(value)),
                Ok(Err(message)) => Err(SemaError::Io(message)),
                Err(failure) => Err(SemaError::eval(failure.message().to_string())),
            },
            Err(failure) => Err(SemaError::eval(format!(
                "{}: {}",
                self.op,
                failure.message()
            ))),
        }
    }
}

/// Resumes the parked file-op frame once the worker completes: the decoded value
/// is injected onto its stack top; a failure or cancellation is raised at the
/// call site (catchable by an enclosing try/catch).
struct FsContinuation {
    op: &'static str,
}

impl Trace for FsContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for FsContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{} was cancelled ({reason:?})",
                self.op
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{} continuation received an unexpected runtime response",
                self.op
            ))),
        }
    }
}

/// Build a quarantined-bounded external file operation and RETURN it as a
/// `NativeOutcome::Suspend` on the runtime native ABI. The runtime submits `job`
/// to the thread-pool executor (so it runs off the VM thread and overlaps sibling
/// work) and, when the worker completes, resumes this frame with the decoded
/// value. `job` is `Send` and returns `Result<T, String>` (Ok payload / domain
/// I/O error).
fn fs_quarantined<T, F>(op: &'static str, to_value: fn(T) -> Value, job: F) -> NativeResult
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let kind =
        CompletionKind::try_from_raw(FS_COMPLETION_KIND).expect("file completion kind is nonzero");
    let bound = QuarantineBound::hard_deadline(FS_CLEANUP_DEADLINE)
        .expect("file cleanup deadline is nonzero");
    let prepared = PreparedExternalOperation::quarantined_blocking(
        kind,
        Box::new(FsDecoder { op, to_value }),
        bound,
        move || {
            let inflight = FS_INFLIGHT.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            FS_PEAK_INFLIGHT.fetch_max(inflight, AtomicOrdering::SeqCst);
            let delay = FS_TEST_DELAY_MS.load(AtomicOrdering::Relaxed);
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            let result = job();
            FS_INFLIGHT.fetch_sub(1, AtomicOrdering::SeqCst);
            Ok(Box::new(result) as SendPayload)
        },
    );
    let suspend = NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(FsContinuation { op }),
    };
    Ok(NativeOutcome::Suspend(suspend))
}

/// Decodes a quarantined COMPUTE job's send-safe payload back into a `Value`.
/// Unlike [`FsDecoder`], a domain error (the job's `Err(String)`) is surfaced as
/// `SemaError::eval` rather than `SemaError::Io`. The CPU-bound archive/pdf/diff/
/// server-file ops already build their own op-prefixed error strings (often the
/// full `Display` of a `SemaError`), so eval-wrapping preserves their
/// user-visible error contract.
struct ComputeDecoder<T: Send + 'static> {
    op: &'static str,
    to_value: fn(T) -> Value,
}

impl<T: Send + 'static> Trace for ComputeDecoder<T> {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl<T: Send + 'static> CompletionDecoder for ComputeDecoder<T> {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        match result {
            Ok(payload) => match downcast_send_payload::<Result<T, String>>(payload, self.op) {
                Ok(Ok(value)) => Ok((self.to_value)(value)),
                Ok(Err(message)) => Err(SemaError::eval(message)),
                Err(failure) => Err(SemaError::eval(failure.message().to_string())),
            },
            Err(failure) => Err(SemaError::eval(format!(
                "{}: {}",
                self.op,
                failure.message()
            ))),
        }
    }
}

/// Like [`fs_quarantined`], but for a pure CPU-bound compute (archive/pdf/diff/
/// server-file) whose domain errors are surfaced through `SemaError::eval` (see
/// [`ComputeDecoder`]). The `job` runs quarantined-bounded on the thread-pool
/// executor (overlapping siblings) and the decoded value resumes the parked
/// frame. `job` is `Send` and returns `Result<T, String>` (Ok payload /
/// pre-rendered domain error); `to_value` decodes the `Ok` payload on the VM
/// thread. Cancellation is best-effort (the bounded job runs to completion and
/// its result is discarded), matching `fs_offload`'s no-abort-hook policy.
pub(crate) fn quarantined_compute<T, F>(
    op: &'static str,
    to_value: fn(T) -> Value,
    job: F,
) -> NativeResult
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let kind = CompletionKind::try_from_raw(FS_COMPLETION_KIND)
        .expect("compute completion kind is nonzero");
    let bound = QuarantineBound::hard_deadline(FS_CLEANUP_DEADLINE)
        .expect("compute cleanup deadline is nonzero");
    let prepared = PreparedExternalOperation::quarantined_blocking(
        kind,
        Box::new(ComputeDecoder { op, to_value }),
        bound,
        move || Ok(Box::new(job()) as SendPayload),
    );
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(FsContinuation { op }),
    }))
}

/// Like [`quarantined_compute`], but the result is decoded by a caller-supplied
/// `decoder` that MAY hold Sema `Value`s across the park (e.g. `http/router`'s
/// route handlers, which must be rebuilt into the dispatch fn once the static
/// directories canonicalize off-thread). The decoder implements
/// [`CompletionDecoder`] (whose `Trace` supertrait exposes those `Value` edges to
/// the collector, so nothing it holds is reclaimed while the job is in flight)
/// and turns the job's `Send` payload into the resume `Value`.
pub(crate) fn quarantined_compute_with_decoder<F>(
    op: &'static str,
    decoder: Box<dyn CompletionDecoder>,
    job: F,
) -> NativeResult
where
    F: FnOnce() -> Result<SendPayload, ExternalFailure> + Send + 'static,
{
    let kind = CompletionKind::try_from_raw(FS_COMPLETION_KIND)
        .expect("compute completion kind is nonzero");
    let bound = QuarantineBound::hard_deadline(FS_CLEANUP_DEADLINE)
        .expect("compute cleanup deadline is nonzero");
    let prepared = PreparedExternalOperation::quarantined_blocking(kind, decoder, bound, job);
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(FsContinuation { op }),
    }))
}

/// Enforce the finite-file byte cap on the VM thread BEFORE dispatch. `stat`s the
/// path and rejects an oversized file with a Sema condition (never an unbounded
/// worker allocation). A missing/unstattable path is NOT rejected here — the job
/// surfaces the real I/O error, matching the synchronous path's behavior.
fn fs_byte_cap_check(op: &str, path: &str) -> Result<(), SemaError> {
    let cap = FS_BYTE_CAP.load(AtomicOrdering::SeqCst);
    if let Ok(meta) = std::fs::metadata(path) {
        let len = meta.len();
        if len > cap {
            return Err(SemaError::eval(format!(
                "{op} {path}: file size {len} bytes exceeds the {cap}-byte quarantined read cap"
            ))
            .with_hint("read the file in bounded chunks or raise the quarantined byte cap"));
        }
    }
    Ok(())
}

/// Enforce the finite-file byte cap on an in-memory write payload before dispatch.
fn fs_write_cap_check(op: &str, len: usize) -> Result<(), SemaError> {
    let cap = FS_BYTE_CAP.load(AtomicOrdering::SeqCst);
    if len as u64 > cap {
        return Err(SemaError::eval(format!(
            "{op}: payload {len} bytes exceeds the {cap}-byte quarantined write cap"
        ))
        .with_hint("write the file in bounded chunks or raise the quarantined byte cap"));
    }
    Ok(())
}

// ── Cooperative line streaming (file/for-each-line, file/fold-lines[-bytes]) ─
//
// The file reader is moved to a blocking worker for one bounded batch of lines,
// then returned to the VM thread while each Sema callback is driven as a
// structural `NativeOutcome::Call`. A callback may therefore park on a timer,
// channel, or nested async operation without blocking sibling tasks. Only raw
// strings/bytes and the `BufReader` cross the worker boundary; every live Sema
// `Value` stays in a traced continuation on the VM thread. Cancellation drops
// that continuation (and its reader) or discards a still-running bounded read.

const FILE_LINE_CHUNK_MAX_LINES: usize = 256;
const FILE_LINE_CHUNK_MAX_BYTES: usize = 256 * 1024;
const FILE_LINE_READER_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
enum FileLineKind {
    ForEachText,
    FoldText,
    FoldBytes,
}

impl FileLineKind {
    fn op(self) -> &'static str {
        match self {
            Self::ForEachText => "file/for-each-line",
            Self::FoldText => "file/fold-lines",
            Self::FoldBytes => "file/fold-lines-bytes",
        }
    }

    fn is_bytes(self) -> bool {
        matches!(self, Self::FoldBytes)
    }
}

enum FileLineItem {
    Text(String),
    Bytes(Vec<u8>),
}

impl FileLineItem {
    fn into_value(self) -> Value {
        match self {
            Self::Text(line) => Value::string_owned(line),
            Self::Bytes(line) => Value::bytevector(line),
        }
    }
}

struct FileLineReader {
    reader: std::io::BufReader<std::fs::File>,
    pending_line: Vec<u8>,
    batch_lines: bool,
}

struct FileLineChunk {
    reader: FileLineReader,
    lines: std::collections::VecDeque<FileLineItem>,
    eof: bool,
    terminal_error: Option<String>,
}

fn finish_file_line(
    pending_line: &mut Vec<u8>,
    terminated: bool,
    bytes: bool,
    op: &str,
    path: &str,
) -> Result<FileLineItem, String> {
    if terminated && pending_line.last() == Some(&b'\r') {
        pending_line.pop();
    }
    let line = std::mem::take(pending_line);
    if bytes {
        Ok(FileLineItem::Bytes(line))
    } else {
        String::from_utf8(line)
            .map(FileLineItem::Text)
            .map_err(|_| format!("{op} {path}: stream did not contain valid UTF-8"))
    }
}

fn oversized_file_line_error(op: &str, path: &str) -> String {
    format!(
        "{op} {path}: line exceeds the {FILE_LINE_CHUNK_MAX_BYTES}-byte cooperative streaming limit"
    )
}

fn read_file_line_chunk(
    op: &'static str,
    path: &str,
    reader: Option<FileLineReader>,
    bytes: bool,
) -> Result<FileLineChunk, String> {
    let mut state = match reader {
        Some(reader) => reader,
        None => {
            let file =
                std::fs::File::open(path).map_err(|error| format!("{op} {path}: {error}"))?;
            let batch_lines = file.metadata().is_ok_and(|metadata| metadata.is_file());
            FileLineReader {
                reader: std::io::BufReader::with_capacity(FILE_LINE_READER_BUFFER_BYTES, file),
                pending_line: Vec::with_capacity(128),
                batch_lines,
            }
        }
    };
    let mut lines = std::collections::VecDeque::with_capacity(FILE_LINE_CHUNK_MAX_LINES);
    let mut bytes_read = 0usize;
    let mut eof = false;
    let mut terminal_error = None;

    while lines.len() < FILE_LINE_CHUNK_MAX_LINES && bytes_read < FILE_LINE_CHUNK_MAX_BYTES {
        let chunk_remaining = FILE_LINE_CHUNK_MAX_BYTES - bytes_read;
        let pending_len = state.pending_line.len();
        let (consume, complete) = {
            let available = match state.reader.fill_buf() {
                Ok(available) => available,
                Err(error) => {
                    terminal_error = Some(format!("{op} {path}: {error}"));
                    break;
                }
            };
            if available.is_empty() {
                eof = true;
                if state.pending_line.len() > FILE_LINE_CHUNK_MAX_BYTES {
                    terminal_error = Some(oversized_file_line_error(op, path));
                } else if !state.pending_line.is_empty() {
                    match finish_file_line(&mut state.pending_line, false, bytes, op, path) {
                        Ok(line) => lines.push_back(line),
                        Err(error) => terminal_error = Some(error),
                    }
                }
                break;
            }

            let scan_len = available.len().min(chunk_remaining);
            match available[..scan_len].iter().position(|byte| *byte == b'\n') {
                Some(newline) => {
                    let trailing_cr = if newline > 0 {
                        available[newline - 1] == b'\r'
                    } else {
                        state.pending_line.last() == Some(&b'\r')
                    };
                    let content_len = pending_len + newline - usize::from(trailing_cr);
                    if content_len > FILE_LINE_CHUNK_MAX_BYTES {
                        terminal_error = Some(oversized_file_line_error(op, path));
                        break;
                    }
                    state.pending_line.extend_from_slice(&available[..newline]);
                    (newline + 1, true)
                }
                None => {
                    // Keep one provisional CR beyond the content limit until
                    // the next byte proves it is a CRLF terminator. EOF or any
                    // non-LF successor leaves it as oversized line content.
                    let storage_remaining =
                        (FILE_LINE_CHUNK_MAX_BYTES + 1).saturating_sub(state.pending_line.len());
                    if scan_len > storage_remaining
                        || (pending_len + scan_len > FILE_LINE_CHUNK_MAX_BYTES
                            && available[scan_len - 1] != b'\r')
                    {
                        terminal_error = Some(oversized_file_line_error(op, path));
                        break;
                    }
                    state.pending_line.extend_from_slice(&available[..scan_len]);
                    (scan_len, false)
                }
            }
        };
        state.reader.consume(consume);
        bytes_read += consume;

        if complete {
            match finish_file_line(&mut state.pending_line, true, bytes, op, path) {
                Ok(line) => lines.push_back(line),
                Err(error) => {
                    terminal_error = Some(error);
                    break;
                }
            }
            if !state.batch_lines {
                break;
            }
        }
    }

    Ok(FileLineChunk {
        reader: state,
        lines,
        eof,
        terminal_error,
    })
}

struct FileLineChunkDecoder {
    op: &'static str,
    slot: std::rc::Rc<std::cell::RefCell<Option<FileLineChunk>>>,
}

impl Trace for FileLineChunkDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for FileLineChunkDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        match result {
            Ok(payload) => {
                match downcast_send_payload::<Result<FileLineChunk, String>>(payload, self.op) {
                    Ok(Ok(chunk)) => {
                        *self.slot.borrow_mut() = Some(chunk);
                        Ok(Value::nil())
                    }
                    Ok(Err(message)) => Err(SemaError::Io(message)),
                    Err(failure) => Err(SemaError::eval(failure.message().to_string())),
                }
            }
            Err(failure) => Err(SemaError::eval(format!(
                "{}: {}",
                self.op,
                failure.message()
            ))),
        }
    }
}

enum FileLineResult {
    ForEach,
    Fold(Value),
}

impl FileLineResult {
    fn callback_args(&mut self, line: Value) -> Vec<Value> {
        match self {
            Self::ForEach => vec![line],
            Self::Fold(accumulator) => {
                vec![std::mem::replace(accumulator, Value::nil()), line]
            }
        }
    }

    fn accept_callback_result(&mut self, value: Value) {
        if let Self::Fold(accumulator) = self {
            *accumulator = value;
        }
    }

    fn finish(self) -> Value {
        match self {
            Self::ForEach => Value::nil(),
            Self::Fold(accumulator) => accumulator,
        }
    }

    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        if let Self::Fold(accumulator) = self {
            sink(GcEdge::Value(accumulator));
        }
    }
}

struct FileLineContinuation {
    kind: FileLineKind,
    path: String,
    callback: Value,
    result: FileLineResult,
    reader: Option<FileLineReader>,
    lines: std::collections::VecDeque<FileLineItem>,
    eof: bool,
    terminal_error: Option<String>,
    read_slot: Option<std::rc::Rc<std::cell::RefCell<Option<FileLineChunk>>>>,
}

impl Trace for FileLineContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.callback));
        self.result.trace(sink);
        true
    }
}

impl FileLineContinuation {
    fn start(
        kind: FileLineKind,
        path: String,
        callback: Value,
        result: FileLineResult,
    ) -> NativeResult {
        Box::new(Self {
            kind,
            path,
            callback,
            result,
            reader: None,
            lines: std::collections::VecDeque::new(),
            eof: false,
            terminal_error: None,
            read_slot: None,
        })
        .suspend_read()
    }

    fn suspend_read(mut self: Box<Self>) -> NativeResult {
        let op = self.kind.op();
        let path = self.path.clone();
        let reader = self.reader.take();
        let bytes = self.kind.is_bytes();
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        self.read_slot = Some(slot.clone());

        let kind = CompletionKind::try_from_raw(FS_COMPLETION_KIND)
            .expect("file completion kind is nonzero");
        let bound = QuarantineBound::hard_deadline(FS_CLEANUP_DEADLINE)
            .expect("file cleanup deadline is nonzero");
        let prepared = PreparedExternalOperation::quarantined_blocking(
            kind,
            Box::new(FileLineChunkDecoder { op, slot }),
            bound,
            move || {
                let inflight = FS_INFLIGHT.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                FS_PEAK_INFLIGHT.fetch_max(inflight, AtomicOrdering::SeqCst);
                let delay = FS_TEST_DELAY_MS.load(AtomicOrdering::Relaxed);
                if delay > 0 {
                    std::thread::sleep(Duration::from_millis(delay));
                }
                let result = read_file_line_chunk(op, &path, reader, bytes);
                FS_INFLIGHT.fetch_sub(1, AtomicOrdering::SeqCst);
                Ok(Box::new(result) as SendPayload)
            },
        );
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::External(Box::new(prepared)),
            continuation: self,
        }))
    }

    fn continue_iteration(mut self: Box<Self>) -> NativeResult {
        if let Some(line) = self.lines.pop_front() {
            let args = self.result.callback_args(line.into_value());
            return Ok(NativeOutcome::Call(sema_core::runtime::NativeCall {
                callable: self.callback.clone(),
                args,
                continuation: self,
            }));
        }
        if let Some(error) = self.terminal_error.take() {
            return Err(SemaError::Io(error));
        }
        if self.eof {
            return Ok(NativeOutcome::Return(self.result.finish()));
        }
        self.suspend_read()
    }
}

impl NativeContinuation for FileLineContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        if let Some(slot) = self.read_slot.take() {
            match input {
                ResumeInput::Returned(_) => {}
                ResumeInput::Failed(error) => return Err(error),
                ResumeInput::Cancelled(reason) => {
                    return Err(SemaError::eval(format!(
                        "{} was cancelled ({reason:?})",
                        self.kind.op()
                    )))
                }
                ResumeInput::Runtime(_) => {
                    return Err(SemaError::eval(format!(
                        "{} continuation received an unexpected runtime response",
                        self.kind.op()
                    )))
                }
            }
            let chunk = slot.borrow_mut().take().ok_or_else(|| {
                SemaError::eval(format!(
                    "{} completion did not return its line reader",
                    self.kind.op()
                ))
            })?;
            self.reader = Some(chunk.reader);
            self.lines = chunk.lines;
            self.eof = chunk.eof;
            self.terminal_error = chunk.terminal_error;
        } else {
            let value = crate::list::resume_value(input, self.kind.op())?;
            self.result.accept_callback_result(value);
        }
        self.continue_iteration()
    }
}

fn file_line_runtime(args: &[Value], kind: FileLineKind) -> NativeResult {
    let op = kind.op();
    match kind {
        FileLineKind::ForEachText => check_arity!(args, op, 2),
        FileLineKind::FoldText | FileLineKind::FoldBytes => check_arity!(args, op, 3),
    }
    let path = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
        .to_string();
    let callback = args[1].clone();
    let result = match kind {
        FileLineKind::ForEachText => FileLineResult::ForEach,
        FileLineKind::FoldText | FileLineKind::FoldBytes => FileLineResult::Fold(args[2].clone()),
    };
    FileLineContinuation::start(kind, path, callback, result)
}

/// Crate-internal: poll stdin for a key within `ms` and decode it, for
/// `event/select`'s `:key` source. Returns the key event, or `None` if no key
/// is ready (or on non-unix platforms, where raw key input isn't wired).
#[cfg(unix)]
pub(crate) fn poll_key_event(ms: u64) -> Option<Value> {
    read_key_from_owner(Some(Duration::from_millis(ms)))
        .ok()
        .filter(|value| !value.is_nil())
}

#[cfg(not(unix))]
pub(crate) fn poll_key_event(_ms: u64) -> Option<Value> {
    None
}

// ── Cooperative runtime poll (event/select, io/read-key-timeout) ─────────────
//
// The unified-runtime analog of [`await_io_until`]: `io/read-key-timeout` and
// `event/select` must poll a readiness source that lives ENTIRELY on the VM
// thread (stdin, a `proc/*` handle registry, elapsed timers) — none of it is
// `Send`, so it can't move onto a worker like a file read. The runtime path
// re-checks the source on the VM thread and parks on a structural timer between
// scans, so sibling tasks run while it waits. The readiness probe (which may hold
// Sema `Value`s — e.g. `event/select`'s source maps) rides in the resume
// continuation and is GC-traced, unlike `await_io_until`'s boxed closure.

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(crate) enum RuntimePollResult {
    Ready(Value),
    Failed(String),
    PendingAfter(Duration),
}

/// A VM-thread readiness probe for [`await_runtime_until`]. Runs entirely on the
/// VM thread each scan, so it MAY hold Sema `Value`s (`event/select`'s source
/// maps); those are live GC edges while the poll is parked and are traced via the
/// `Trace` supertrait — the runtime can account them, unlike the legacy untraced
/// `IoHandle`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) trait RuntimePoll: Trace {
    fn poll(&mut self) -> RuntimePollResult;
}

/// Cooperatively poll `probe` under the unified runtime until it yields a value
/// or `timeout_ms` milliseconds have elapsed since `started` (then nil), yielding
/// a structural timer between scans so siblings overlap. Returns on the runtime
/// native ABI (`NativeOutcome::Suspend`/`Return`). The runtime-native twin of
/// [`await_io_until`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn await_runtime_until(
    probe: Box<dyn RuntimePoll>,
    started: std::time::Instant,
    timeout_ms: u64,
) -> NativeResult {
    runtime_poll_step(probe, Some((started, Duration::from_millis(timeout_ms))))
}

#[cfg(not(target_arch = "wasm32"))]
fn await_runtime_indefinitely(probe: Box<dyn RuntimePoll>) -> NativeResult {
    runtime_poll_step(probe, None)
}

/// One scan of the cooperative poll: check the probe on the VM thread; resolve if
/// ready, resolve to nil if the deadline passed, else park until the probe's next
/// useful check or the overall timeout.
#[cfg(not(target_arch = "wasm32"))]
fn runtime_poll_step(
    mut probe: Box<dyn RuntimePoll>,
    deadline: Option<(std::time::Instant, Duration)>,
) -> NativeResult {
    let requested_delay = match probe.poll() {
        RuntimePollResult::Ready(value) => return Ok(NativeOutcome::Return(value)),
        RuntimePollResult::Failed(message) => return Err(SemaError::eval(message)),
        RuntimePollResult::PendingAfter(delay) => delay,
    };

    let delay = if let Some((started, timeout)) = deadline {
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Ok(NativeOutcome::Return(Value::nil()));
        };
        if remaining.is_zero() {
            return Ok(NativeOutcome::Return(Value::nil()));
        }
        requested_delay.min(remaining)
    } else {
        requested_delay
    };

    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::Timer(delay),
        continuation: Box::new(RuntimePollContinuation {
            probe: Some(probe),
            deadline,
        }),
    }))
}

/// Resumes a parked cooperative poll after its structural timer elapses: re-check
/// the probe (start the next scan, or resolve). Carries the `RuntimePoll` probe
/// across the park and traces any `Value` it holds.
#[cfg(not(target_arch = "wasm32"))]
struct RuntimePollContinuation {
    probe: Option<Box<dyn RuntimePoll>>,
    deadline: Option<(std::time::Instant, Duration)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Trace for RuntimePollContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match &self.probe {
            Some(probe) => probe.trace(sink),
            None => true,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeContinuation for RuntimePollContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(_) => {
                let probe = self.probe.take().expect("runtime poll probe resumed once");
                runtime_poll_step(probe, self.deadline)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "cooperative poll was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "cooperative poll continuation received an unexpected runtime response",
            )),
        }
    }
}

/// The VM-thread readiness probe for `io/read-key-timeout`: a keypress is ready
/// once a byte is available on stdin; EOF resolves to nil. Holds no `Value`.
#[cfg(unix)]
struct KeyProbe {
    lease: crate::stream::StdinInputLease,
    bytes: Vec<u8>,
    completed_bytes: Vec<u8>,
    escape_started: Option<std::time::Instant>,
    terminal: bool,
}

#[cfg(unix)]
impl KeyProbe {
    fn new() -> Self {
        Self {
            lease: crate::stream::acquire_stdin_input(),
            bytes: Vec::new(),
            completed_bytes: Vec::new(),
            escape_started: None,
            terminal: false,
        }
    }

    fn take_completed_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.completed_bytes)
    }

    fn next_event(&mut self) {
        debug_assert!(self.bytes.is_empty());
        debug_assert!(self.completed_bytes.is_empty());
        self.escape_started = None;
        self.terminal = false;
    }
}

#[cfg(unix)]
impl Drop for KeyProbe {
    fn drop(&mut self) {
        if !self.terminal {
            self.lease.return_bytes(&self.bytes);
        }
    }
}

#[cfg(unix)]
const RUNTIME_KEY_BYTE_CAP: usize = 8 * 1024 * 1024;

#[cfg(unix)]
fn utf8_event_width(lead: u8) -> usize {
    if lead & 0xe0 == 0xc0 {
        2
    } else if lead & 0xf0 == 0xe0 {
        3
    } else if lead & 0xf8 == 0xf0 {
        4
    } else {
        1
    }
}

#[cfg(unix)]
fn runtime_key_complete(bytes: &[u8], escape_started: Option<std::time::Instant>) -> bool {
    let Some(first) = bytes.first().copied() else {
        return false;
    };
    if first != 0x1b {
        return bytes.len() >= utf8_event_width(first);
    }
    let Some(second) = bytes.get(1).copied() else {
        return escape_started
            .is_some_and(|started| started.elapsed() >= Duration::from_millis(50));
    };
    if second == b'[' {
        let csi = &bytes[2..];
        if csi.starts_with(b"200~") {
            return csi.ends_with(b"\x1b[201~");
        }
        return csi.iter().any(|byte| (0x40..=0x7e).contains(byte));
    }
    if second == b'O' {
        return bytes.len() >= 3;
    }
    bytes.len() > utf8_event_width(second)
}

#[cfg(unix)]
fn runtime_key_poll_delay(bytes: &[u8], escape_started: Option<std::time::Instant>) -> Duration {
    if bytes == [0x1b] {
        let elapsed = escape_started.map_or(Duration::ZERO, |started| started.elapsed());
        return Duration::from_millis(50).saturating_sub(elapsed);
    }
    Duration::from_millis(5)
}

#[cfg(unix)]
fn poll_runtime_key(probe: &mut KeyProbe) -> RuntimePollResult {
    loop {
        if runtime_key_complete(&probe.bytes, probe.escape_started) {
            probe.terminal = true;
            probe.completed_bytes = std::mem::take(&mut probe.bytes);
            return RuntimePollResult::Ready(decode_runtime_key(&probe.completed_bytes));
        }
        match probe.lease.poll(1, None, "io/read-key") {
            Ok(crate::stream::StdinInputPoll::Data(bytes)) => {
                let byte = bytes[0];
                if probe.bytes.is_empty() && byte == 0x1b {
                    probe.escape_started = Some(std::time::Instant::now());
                }
                probe.bytes.push(byte);
                if probe.bytes.len() > RUNTIME_KEY_BYTE_CAP {
                    probe.terminal = true;
                    return RuntimePollResult::Failed(format!(
                        "io/read-key: input exceeds the {RUNTIME_KEY_BYTE_CAP}-byte event cap"
                    ));
                }
            }
            Ok(crate::stream::StdinInputPoll::Eof) if probe.bytes.is_empty() => {
                probe.terminal = true;
                mark_stdin_eof();
                return RuntimePollResult::Ready(Value::nil());
            }
            Ok(crate::stream::StdinInputPoll::Eof) => {
                probe.terminal = true;
                mark_stdin_eof();
                probe.completed_bytes = std::mem::take(&mut probe.bytes);
                return RuntimePollResult::Ready(decode_runtime_key(&probe.completed_bytes));
            }
            Ok(crate::stream::StdinInputPoll::Pending) => {
                return RuntimePollResult::PendingAfter(runtime_key_poll_delay(
                    &probe.bytes,
                    probe.escape_started,
                ));
            }
            Err(error) => {
                probe.terminal = true;
                return RuntimePollResult::Failed(error.to_string());
            }
        }
    }
}

#[cfg(unix)]
impl Trace for KeyProbe {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(unix)]
impl RuntimePoll for KeyProbe {
    fn poll(&mut self) -> RuntimePollResult {
        poll_runtime_key(self)
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum TerminalQueryKind {
    KittySupport,
    CursorPosition,
}

#[cfg(unix)]
impl TerminalQueryKind {
    fn fallback(self) -> Value {
        match self {
            Self::KittySupport => Value::bool(false),
            Self::CursorPosition => Value::nil(),
        }
    }
}

#[cfg(unix)]
struct TerminalQueryProbe {
    key: KeyProbe,
    deferred_bytes: Vec<u8>,
    kind: TerminalQueryKind,
    cpr_armed: bool,
    deadline: std::time::Instant,
}

#[cfg(unix)]
impl TerminalQueryProbe {
    fn new(kind: TerminalQueryKind, timeout: Duration) -> Self {
        let cpr_armed = matches!(kind, TerminalQueryKind::CursorPosition);
        if cpr_armed {
            EXPECT_CPR.with(|counter| counter.set(counter.get().saturating_add(1)));
        }
        Self {
            key: KeyProbe::new(),
            deferred_bytes: Vec::new(),
            kind,
            cpr_armed,
            deadline: std::time::Instant::now() + timeout,
        }
    }

    fn is_kind(value: &Value, kind: &str) -> bool {
        value
            .as_map_ref()
            .and_then(|map| map.get(&Value::keyword("kind")).cloned())
            == Some(Value::keyword(kind))
    }

    fn defer_completed_event(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > RUNTIME_KEY_BYTE_CAP.saturating_sub(self.deferred_bytes.len()) {
            // `prepend` places each call before the previous one. Return the
            // newest event first, then the earlier deferred bytes, so the next
            // stdin consumer observes their original order.
            self.key.lease.return_bytes(&bytes);
            self.key.lease.return_bytes(&self.deferred_bytes);
            self.deferred_bytes.clear();
            self.key.terminal = true;
            return Err(format!(
                "terminal query: unrelated input exceeds the {RUNTIME_KEY_BYTE_CAP}-byte preservation cap"
            ));
        }
        self.deferred_bytes.extend(bytes);
        self.key.next_event();
        Ok(())
    }

    fn requeue_preserved_input(&mut self) {
        let partial = std::mem::take(&mut self.key.bytes);
        self.key.terminal = true;
        // Return the partial suffix first because each prepend call inserts at
        // the front. Deferred complete events must be observed before it.
        self.key.lease.return_bytes(&partial);
        self.key.lease.return_bytes(&self.deferred_bytes);
        self.deferred_bytes.clear();
    }
}

#[cfg(unix)]
impl Drop for TerminalQueryProbe {
    fn drop(&mut self) {
        self.requeue_preserved_input();
        if self.cpr_armed {
            EXPECT_CPR.with(|counter| counter.set(counter.get().saturating_sub(1)));
        }
    }
}

#[cfg(unix)]
impl Trace for TerminalQueryProbe {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.key.trace(sink)
    }
}

#[cfg(unix)]
impl RuntimePoll for TerminalQueryProbe {
    fn poll(&mut self) -> RuntimePollResult {
        loop {
            let Some(remaining) = self
                .deadline
                .checked_duration_since(std::time::Instant::now())
            else {
                return RuntimePollResult::Ready(self.kind.fallback());
            };
            if remaining.is_zero() {
                return RuntimePollResult::Ready(self.kind.fallback());
            }
            match self.key.poll() {
                RuntimePollResult::Ready(value) if value.is_nil() => {
                    return RuntimePollResult::Ready(self.kind.fallback());
                }
                RuntimePollResult::Ready(value) => {
                    let bytes = self.key.take_completed_bytes();
                    match self.kind {
                        TerminalQueryKind::KittySupport if Self::is_kind(&value, "kitty-flags") => {
                            return RuntimePollResult::Ready(Value::bool(true));
                        }
                        TerminalQueryKind::KittySupport
                            if Self::is_kind(&value, "device-attributes") =>
                        {
                            return RuntimePollResult::Ready(Value::bool(false));
                        }
                        TerminalQueryKind::CursorPosition if Self::is_kind(&value, "cpr") => {
                            self.cpr_armed = false;
                            return RuntimePollResult::Ready(value);
                        }
                        _ => {
                            if let Err(message) = self.defer_completed_event(bytes) {
                                return RuntimePollResult::Failed(message);
                            }
                        }
                    }
                }
                RuntimePollResult::Failed(message) => {
                    return RuntimePollResult::Failed(message);
                }
                RuntimePollResult::PendingAfter(delay) => {
                    return RuntimePollResult::PendingAfter(delay.min(remaining));
                }
            }
        }
    }
}

#[cfg(unix)]
fn run_terminal_query_value(
    kind: TerminalQueryKind,
    timeout: Duration,
) -> Result<Value, SemaError> {
    let mut probe = TerminalQueryProbe::new(kind, timeout);
    loop {
        match probe.poll() {
            RuntimePollResult::Ready(value) => return Ok(value),
            RuntimePollResult::Failed(message) => return Err(SemaError::eval(message)),
            RuntimePollResult::PendingAfter(delay) => std::thread::sleep(delay),
        }
    }
}

#[cfg(unix)]
fn terminal_query_runtime(kind: TerminalQueryKind) -> NativeResult {
    terminal_query_runtime_with_timeout(kind, Duration::from_millis(200))
}

#[cfg(unix)]
fn terminal_query_runtime_with_timeout(kind: TerminalQueryKind, timeout: Duration) -> NativeResult {
    await_runtime_indefinitely(Box::new(TerminalQueryProbe::new(kind, timeout)))
}

/// Synchronous value-ABI body for `io/read-key-timeout` using the shared stdin
/// owner. The cooperative path lives in the runtime ABI
/// ([`register_read_key_timeout`]).
#[cfg(unix)]
fn read_key_timeout_value(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "io/read-key-timeout", 1);
    let ms = args[0]
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))? as u64;

    read_key_from_owner(Some(Duration::from_millis(ms)))
}

#[cfg(unix)]
fn read_key_from_owner(timeout: Option<Duration>) -> Result<Value, SemaError> {
    let started = std::time::Instant::now();
    let mut probe = KeyProbe::new();
    loop {
        match poll_runtime_key(&mut probe) {
            RuntimePollResult::Ready(value) => return Ok(value),
            RuntimePollResult::Failed(message) => return Err(SemaError::eval(message)),
            RuntimePollResult::PendingAfter(delay) => {
                let sleep = if let Some(timeout) = timeout {
                    let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                        return Ok(Value::nil());
                    };
                    if remaining.is_zero() {
                        return Ok(Value::nil());
                    }
                    delay.min(remaining)
                } else {
                    delay
                };
                std::thread::sleep(sleep);
            }
        }
    }
}

#[cfg(unix)]
fn read_key_value(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "io/read-key", 0);
    read_key_from_owner(None)
}

#[cfg(unix)]
fn register_read_key(env: &sema_core::Env) {
    crate::register_runtime_fn(env, "io/read-key", |args| {
        if sema_core::in_runtime_quantum() {
            check_arity!(args, "io/read-key", 0);
            return await_runtime_indefinitely(Box::new(KeyProbe::new()));
        }
        read_key_value(args).map(NativeOutcome::Return)
    });
}

/// Register `io/read-key-timeout` dual-ABI: the value body is synchronous; the
/// runtime body uses structural timer wakes so a "key OR agent progress" loop
/// overlaps sibling tasks.
#[cfg(unix)]
fn register_read_key_timeout(env: &sema_core::Env) {
    env.set(
        sema_core::intern("io/read-key-timeout"),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            "io/read-key-timeout",
            read_key_timeout_value,
            |_ctx, args| {
                if sema_core::in_runtime_quantum() {
                    check_arity!(args, "io/read-key-timeout", 1);
                    let ms = args[0]
                        .as_int()
                        .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?
                        as u64;
                    let started = std::time::Instant::now();
                    return await_runtime_until(Box::new(KeyProbe::new()), started, ms);
                }
                read_key_timeout_value(args).map(NativeOutcome::Return)
            },
        )),
    );
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

    crate::register_runtime_fn_path_gated(env, sandbox, Caps::FS_READ, "file/read", &[0], |args| {
        check_arity!(args, "file/read", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if let Some(data) = sema_core::vfs::vfs_read(path) {
            return String::from_utf8(data)
                .map_err(|e| SemaError::Io(format!("file/read {path}: invalid UTF-8 in VFS: {e}")))
                .map(|s| NativeOutcome::Return(Value::string_owned(s)));
        }
        if sema_core::in_runtime_quantum() {
            fs_byte_cap_check("file/read", path)?;
            let path = path.to_string();
            return fs_quarantined("file/read", Value::string_owned, move || {
                std::fs::read_to_string(&path).map_err(|e| format!("file/read {path}: {e}"))
            });
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| SemaError::Io(format!("file/read {path}: {e}")))?;
        Ok(NativeOutcome::Return(Value::string_owned(content)))
    });

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_WRITE,
        "file/write",
        &[0],
        |args| {
            check_arity!(args, "file/write", 2);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let content = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            if sema_core::in_runtime_quantum() {
                fs_write_cap_check("file/write", content.len())?;
                let path = path.to_string();
                let content = content.to_string();
                return fs_quarantined(
                    "file/write",
                    |()| Value::nil(),
                    move || {
                        std::fs::write(&path, &content)
                            .map_err(|e| format!("file/write {path}: {e}"))
                    },
                );
            }
            std::fs::write(path, content)
                .map_err(|e| SemaError::Io(format!("file/write {path}: {e}")))?;
            Ok(NativeOutcome::Return(Value::nil()))
        },
    );

    crate::register_runtime_fn_path_gated(
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
                return Ok(NativeOutcome::Return(Value::bytevector(data)));
            }
            if sema_core::in_runtime_quantum() {
                fs_byte_cap_check("file/read-bytes", path)?;
                let path = path.to_string();
                return fs_quarantined("file/read-bytes", Value::bytevector, move || {
                    std::fs::read(&path).map_err(|e| format!("file/read-bytes {path}: {e}"))
                });
            }
            let bytes = std::fs::read(path)
                .map_err(|e| SemaError::Io(format!("file/read-bytes {path}: {e}")))?;
            Ok(NativeOutcome::Return(Value::bytevector(bytes)))
        },
    );

    crate::register_runtime_fn_path_gated(
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
            if sema_core::in_runtime_quantum() {
                fs_write_cap_check("file/write-bytes", bv.len())?;
                let path = path.to_string();
                let bv = bv.to_vec();
                return fs_quarantined(
                    "file/write-bytes",
                    |()| Value::nil(),
                    move || {
                        std::fs::write(&path, &bv)
                            .map_err(|e| format!("file/write-bytes {path}: {e}"))
                    },
                );
            }
            std::fs::write(path, bv)
                .map_err(|e| SemaError::Io(format!("file/write-bytes {path}: {e}")))?;
            Ok(NativeOutcome::Return(Value::nil()))
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/exists?",
        &[0],
        |args| {
            check_arity!(args, "file/exists?", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            if let Some(exists) = sema_core::vfs::vfs_exists(path) {
                if exists {
                    return Ok(NativeOutcome::Return(Value::bool(true)));
                }
            }
            if sema_core::in_runtime_quantum() {
                let path = path.to_string();
                return fs_quarantined("file/exists?", Value::bool, move || {
                    Ok(std::path::Path::new(&path).exists())
                });
            }
            Ok(NativeOutcome::Return(Value::bool(
                std::path::Path::new(path).exists(),
            )))
        },
    );

    crate::register_runtime_fn(env, "read-line", |args| {
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_runtime_quantum() {
            check_arity!(args, "read-line", 0);
            return crate::stream::stdin_text_line("read-line");
        }
        read_line_value(args).map(NativeOutcome::Return)
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
        append_impl(path, content).map_err(SemaError::Io)?;
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

    crate::register_runtime_fn_path_gated(env, sandbox, Caps::FS_READ, "file/list", &[0], |args| {
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
        fn list_to_value(entries: Vec<String>) -> Value {
            Value::list(entries.into_iter().map(|s| Value::string(&s)).collect())
        }
        if sema_core::in_runtime_quantum() {
            // The entry cap is fixed BEFORE dispatch and stored in the job; the
            // worker aborts with a named bound-exceeded error if the directory
            // holds more, so the quarantined job never allocates unboundedly.
            let cap = FS_LIST_CAP.load(AtomicOrdering::SeqCst);
            let path = path.to_string();
            return fs_quarantined("file/list", list_to_value, move || {
                let mut entries = Vec::new();
                for entry in
                    std::fs::read_dir(&path).map_err(|e| format!("file/list {path}: {e}"))?
                {
                    let entry = entry.map_err(|e| format!("file/list {path}: {e}"))?;
                    if entries.len() as u64 >= cap {
                        return Err(format!(
                            "file/list {path}: directory exceeds the {cap}-entry quarantined list cap"
                        ));
                    }
                    entries.push(entry.file_name().to_string_lossy().into_owned());
                }
                Ok(entries)
            });
        }
        let entries = list_impl(path).map_err(SemaError::Io)?;
        Ok(NativeOutcome::Return(Value::list(
            entries.into_iter().map(|s| Value::string(&s)).collect(),
        )))
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

    crate::register_runtime_fn_path_gated(env, sandbox, Caps::FS_READ, "file/info", &[0], |args| {
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
        if sema_core::in_runtime_quantum() {
            let path = path.to_string();
            return fs_quarantined("file/info", info_to_value, move || info_impl(&path));
        }
        let info = info_impl(path).map_err(SemaError::Io)?;
        Ok(NativeOutcome::Return(info_to_value(info)))
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

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/for-each-line",
        &[0],
        |args| {
            check_arity!(args, "file/for-each-line", 2);
            if sema_core::in_runtime_quantum() {
                return file_line_runtime(args, FileLineKind::ForEachText);
            }
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
                Ok(NativeOutcome::Return(Value::nil()))
            })
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/fold-lines",
        &[0],
        |args| {
            check_arity!(args, "file/fold-lines", 3);
            if sema_core::in_runtime_quantum() {
                return file_line_runtime(args, FileLineKind::FoldText);
            }
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
                Ok(NativeOutcome::Return(acc))
            })
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "file/fold-lines-bytes",
        &[0],
        |args| {
            check_arity!(args, "file/fold-lines-bytes", 3);
            if sema_core::in_runtime_quantum() {
                return file_line_runtime(args, FileLineKind::FoldBytes);
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let func = args[1].clone();
            let mut acc = args[2].clone();
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
                Ok(NativeOutcome::Return(acc))
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

    crate::register_runtime_fn(env, "read-stdin", |args| {
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_runtime_quantum() {
            check_arity!(args, "read-stdin", 0);
            return crate::stream::stdin_text("read-stdin");
        }
        read_stdin_value(args).map(NativeOutcome::Return)
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
        register_read_key(env);

        // io/read-key-timeout — like io/read-key but returns nil if no key arrives within
        // `timeout-ms` milliseconds.
        register_read_key_timeout(env);

        // Capability probes (raw mode required; they round-trip a query + reply).
        // term/supports-kitty-keys? → bool via `CSI ?u` + DSR barrier.
        crate::register_runtime_fn(env, "term/supports-kitty-keys?", |args| {
            check_arity!(args, "term/supports-kitty-keys?", 0);
            if sema_core::in_runtime_quantum() {
                if !stdin_is_tty() {
                    return Ok(NativeOutcome::Return(Value::bool(false)));
                }
                write_stdout("\x1b[?u\x1b[c")?;
                return terminal_query_runtime(TerminalQueryKind::KittySupport);
            }
            probe_kitty_support()
                .map(Value::bool)
                .map(NativeOutcome::Return)
        });
        // term/cursor-position → {:row :col} (or nil) via a DSR round-trip.
        crate::register_runtime_fn(env, "term/cursor-position", |args| {
            check_arity!(args, "term/cursor-position", 0);
            if sema_core::in_runtime_quantum() {
                if !stdin_is_tty() {
                    return Ok(NativeOutcome::Return(Value::nil()));
                }
                write_stdout("\x1b[6n")?;
                return terminal_query_runtime(TerminalQueryKind::CursorPosition);
            }
            query_cursor_position().map(NativeOutcome::Return)
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod runtime_poll_tests {
    use std::time::Instant;

    use super::*;

    struct PendingProbe;

    impl Trace for PendingProbe {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl RuntimePoll for PendingProbe {
        fn poll(&mut self) -> RuntimePollResult {
            RuntimePollResult::PendingAfter(Duration::from_millis(37))
        }
    }

    #[test]
    fn runtime_poll_pending_uses_structural_timer() {
        let outcome = await_runtime_until(Box::new(PendingProbe), Instant::now(), 1_000)
            .expect("pending probe suspends");

        let NativeOutcome::Suspend(suspend) = outcome else {
            panic!("pending probe must suspend");
        };
        match suspend.wait {
            WaitKind::Timer(delay) => assert_eq!(delay, Duration::from_millis(37)),
            WaitKind::External(_) => panic!("poll probe must not occupy an executor worker"),
            _ => panic!("poll probe must use a structural timer"),
        }
    }

    #[test]
    fn runtime_poll_zero_timeout_returns_nil_immediately() {
        let outcome = await_runtime_until(Box::new(PendingProbe), Instant::now(), 0)
            .expect("zero-timeout probe returns");

        let NativeOutcome::Return(value) = outcome else {
            panic!("zero-timeout probe must not suspend");
        };
        assert!(value.is_nil());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod file_line_trace_tests {
    use super::*;

    fn edge_count(trace: &dyn Trace) -> usize {
        let mut count = 0;
        assert!(trace.trace(&mut |_| count += 1));
        count
    }

    #[test]
    fn line_continuation_traces_callback_and_fold_accumulator() {
        let continuation = FileLineContinuation {
            kind: FileLineKind::FoldText,
            path: "fixture".to_string(),
            callback: Value::string("callback"),
            result: FileLineResult::Fold(Value::string("accumulator")),
            reader: None,
            lines: std::collections::VecDeque::from([
                FileLineItem::Text("raw-line".to_string()),
                FileLineItem::Bytes(vec![1, 2, 3]),
            ]),
            eof: false,
            terminal_error: None,
            read_slot: Some(std::rc::Rc::new(std::cell::RefCell::new(None))),
        };

        assert_eq!(edge_count(&continuation), 2);
    }

    #[test]
    fn line_decoder_holds_no_value_edges() {
        let decoder = FileLineChunkDecoder {
            op: "file/fold-lines",
            slot: std::rc::Rc::new(std::cell::RefCell::new(None)),
        };

        assert_eq!(edge_count(&decoder), 0);
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("sema-file-lines-{tag}-{nanos}"))
    }

    #[test]
    fn line_chunk_is_bounded_by_line_count() {
        let path = temp_path("count");
        let contents: String = (0..261).map(|index| format!("line-{index}\n")).collect();
        std::fs::write(&path, contents).expect("write line fixture");
        let path_text = path.to_string_lossy();

        let first = read_file_line_chunk("file/fold-lines", &path_text, None, false)
            .expect("read first bounded chunk");
        assert_eq!(first.lines.len(), 256);
        assert!(!first.eof);
        let second = read_file_line_chunk("file/fold-lines", &path_text, Some(first.reader), false)
            .expect("read remaining lines");
        assert_eq!(second.lines.len(), 5);
        assert!(second.eof);

        std::fs::remove_file(path).expect("remove line fixture");
    }

    #[test]
    fn line_chunk_is_bounded_by_bytes() {
        let path = temp_path("bytes");
        let line = vec![b'x'; 130 * 1024];
        let mut contents = Vec::with_capacity((line.len() + 1) * 3);
        for _ in 0..3 {
            contents.extend_from_slice(&line);
            contents.push(b'\n');
        }
        std::fs::write(&path, contents).expect("write byte fixture");
        let path_text = path.to_string_lossy();

        let first = read_file_line_chunk("file/fold-lines-bytes", &path_text, None, true)
            .expect("read first byte-bounded chunk");
        assert_eq!(first.lines.len(), 1);
        assert!(!first.eof);
        let second = read_file_line_chunk(
            "file/fold-lines-bytes",
            &path_text,
            Some(first.reader),
            true,
        )
        .expect("read remaining byte line");
        assert_eq!(second.lines.len(), 2);
        assert!(second.eof);

        std::fs::remove_file(path).expect("remove byte fixture");
    }
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
