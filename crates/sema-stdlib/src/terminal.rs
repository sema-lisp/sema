use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sema_core::{check_arity, SemaError, Value, ValueView};

use crate::register_fn;

fn wrap_sgr(text: &str, code: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn make_style_fn(env: &sema_core::Env, name: &str, code: &str) {
    let code = code.to_string();
    let fn_name = name.to_string();
    register_fn(env, name, move |args| {
        check_arity!(args, &fn_name, 1);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&wrap_sgr(text, &code)))
    });
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL_MS: u64 = 80;

struct SpinnerHandle {
    stop_flag: Arc<AtomicBool>,
    message: Arc<Mutex<String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

thread_local! {
    static SPINNERS: RefCell<HashMap<i64, SpinnerHandle>> = RefCell::new(HashMap::new());
    static SPINNER_COUNTER: Cell<i64> = const { Cell::new(0) };
}

pub fn register(env: &sema_core::Env) {
    // Modifiers
    make_style_fn(env, "term/bold", "1");
    make_style_fn(env, "term/dim", "2");
    make_style_fn(env, "term/italic", "3");
    make_style_fn(env, "term/underline", "4");
    make_style_fn(env, "term/inverse", "7");
    make_style_fn(env, "term/strikethrough", "9");

    // Foreground colors
    make_style_fn(env, "term/black", "30");
    make_style_fn(env, "term/red", "31");
    make_style_fn(env, "term/green", "32");
    make_style_fn(env, "term/yellow", "33");
    make_style_fn(env, "term/blue", "34");
    make_style_fn(env, "term/magenta", "35");
    make_style_fn(env, "term/cyan", "36");
    make_style_fn(env, "term/white", "37");
    make_style_fn(env, "term/gray", "90");

    // (term/style "text" :bold :red ...)
    register_fn(env, "term/style", |args| {
        check_arity!(args, "term/style", 1..);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        let mut codes: Vec<&str> = Vec::new();
        for arg in &args[1..] {
            let kw = arg
                .as_keyword()
                .ok_or_else(|| SemaError::type_error("keyword", arg.type_name()))?;
            let code = match kw.as_str() {
                // Modifiers
                "bold" => "1",
                "dim" => "2",
                "italic" => "3",
                "underline" => "4",
                "inverse" => "7",
                "strikethrough" => "9",
                // Colors
                "black" => "30",
                "red" => "31",
                "green" => "32",
                "yellow" => "33",
                "blue" => "34",
                "magenta" => "35",
                "cyan" => "36",
                "white" => "37",
                "gray" => "90",
                other => {
                    return Err(SemaError::eval(format!(
                        "term/style: unknown style keyword :{other}"
                    )))
                }
            };
            codes.push(code);
        }
        if codes.is_empty() {
            return Ok(Value::string(text));
        }
        let combined = codes.join(";");
        Ok(Value::string(&wrap_sgr(text, &combined)))
    });

    // (term/strip "ansi-string") -> plain string
    register_fn(env, "term/strip", |args| {
        check_arity!(args, "term/strip", 1);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Strip ANSI escape sequences (full CSI + OSC, not just SGR). Shared with
        // string/width & string/wrap via crate::strip_ansi.
        Ok(Value::string(&crate::strip_ansi(text)))
    });

    // (term/rgb "text" r g b) -> 24-bit color
    register_fn(env, "term/rgb", |args| {
        check_arity!(args, "term/rgb", 4);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let r = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
        let g = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))?;
        let b = args[3]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[3].type_name()))?;
        Ok(Value::string(&format!(
            "\x1b[38;2;{r};{g};{b}m{text}\x1b[0m"
        )))
    });

    // (term/spinner-start "message") -> spinner-id
    register_fn(env, "term/spinner-start", |args| {
        check_arity!(args, "term/spinner-start", 1);
        let msg = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();

        let id = SPINNER_COUNTER.with(|c| {
            let id = c.get();
            c.set(id + 1);
            id
        });

        let stop_flag = Arc::new(AtomicBool::new(false));
        let message = Arc::new(Mutex::new(msg));

        let stop_clone = Arc::clone(&stop_flag);
        let msg_clone = Arc::clone(&message);

        let thread = std::thread::spawn(move || {
            let mut frame_idx = 0usize;
            loop {
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }
                let msg = msg_clone.lock().unwrap().clone();
                let frame = SPINNER_FRAMES[frame_idx % SPINNER_FRAMES.len()];
                // Write spinner frame to stderr
                let mut stderr = std::io::stderr().lock();
                let _ = write!(stderr, "\r\x1b[K{frame} {msg}");
                let _ = stderr.flush();
                drop(stderr);
                frame_idx += 1;
                std::thread::sleep(std::time::Duration::from_millis(SPINNER_INTERVAL_MS));
            }
        });

        SPINNERS.with(|spinners| {
            spinners.borrow_mut().insert(
                id,
                SpinnerHandle {
                    stop_flag,
                    message,
                    thread: Some(thread),
                },
            );
        });

        Ok(Value::int(id))
    });

    // (term/spinner-stop id) or (term/spinner-stop id {:symbol "✔" :text "Done" :color :green})
    register_fn(env, "term/spinner-stop", |args| {
        check_arity!(args, "term/spinner-stop", 1..=2);
        let id = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;

        // Signal stop and wait for thread
        SPINNERS.with(|spinners| {
            let mut map = spinners.borrow_mut();
            if let Some(mut handle) = map.remove(&id) {
                handle.stop_flag.store(true, Ordering::Relaxed);
                if let Some(thread) = handle.thread.take() {
                    let _ = thread.join();
                }

                // Clear the spinner line
                let mut stderr = std::io::stderr().lock();
                let _ = write!(stderr, "\r\x1b[K");

                // Print final status if options provided
                if args.len() == 2 {
                    if let ValueView::Map(opts) = args[1].view() {
                        let symbol = opts
                            .get(&Value::keyword("symbol"))
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        let text = opts
                            .get(&Value::keyword("text"))
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        if !symbol.is_empty() || !text.is_empty() {
                            let _ = writeln!(stderr, "{symbol} {text}");
                        }
                    }
                }
                let _ = stderr.flush();
            }
        });

        Ok(Value::nil())
    });

    // (term/spinner-update id "new message")
    register_fn(env, "term/spinner-update", |args| {
        check_arity!(args, "term/spinner-update", 2);
        let id = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        let new_msg = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        SPINNERS.with(|spinners| {
            let map = spinners.borrow();
            if let Some(handle) = map.get(&id) {
                *handle.message.lock().unwrap() = new_msg;
            }
        });

        Ok(Value::nil())
    });

    register_screen_control(env);
}

// ── Screen control ──────────────────────────────────────────────────────────
//
// Raw terminal screen primitives that emit ANSI/VT control sequences to stdout
// so Sema TUIs (e.g. Sema Coder) don't have to hand-write escape codes. Each
// write is flushed immediately so the effect is visible at human interaction
// speed; render loops can still batch styled strings and call `term/flush`.

/// Write a control sequence to stdout and flush it. Control codes are useless
/// if they sit in a block buffer, so flush-per-call keeps behavior predictable.
fn emit(seq: &str) -> Result<Value, SemaError> {
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())
        .and_then(|_| out.flush())
        .map_err(|e| SemaError::Io(format!("term: {e}")))?;
    Ok(Value::nil())
}

/// Register a zero-arg fn that emits a fixed control sequence.
fn make_emit_fn(env: &sema_core::Env, name: &str, seq: &'static str) {
    let fn_name = name.to_string();
    register_fn(env, name, move |args| {
        check_arity!(args, &fn_name, 0);
        emit(seq)
    });
}

/// 1-based row/col argument → terminal coordinate, clamped to ≥ 1 (VT cursor
/// addressing is 1-based; 0 is treated as 1 by most terminals but we normalize).
fn coord(arg: &Value) -> Result<i64, SemaError> {
    let n = arg
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer", arg.type_name()))?;
    Ok(n.max(1))
}

fn register_screen_control(env: &sema_core::Env) {
    // Alternate screen buffer — enter on app start, leave to restore the user's
    // scrollback exactly as it was.
    make_emit_fn(env, "term/enter-alt-screen", "\x1b[?1049h");
    make_emit_fn(env, "term/leave-alt-screen", "\x1b[?1049l");

    // Clearing
    make_emit_fn(env, "term/clear", "\x1b[2J\x1b[H"); // whole screen + home
    make_emit_fn(env, "term/clear-line", "\x1b[2K"); // current line
    make_emit_fn(env, "term/clear-below", "\x1b[0J"); // cursor → end of screen

    // Cursor
    make_emit_fn(env, "term/cursor-home", "\x1b[H");
    make_emit_fn(env, "term/hide-cursor", "\x1b[?25l");
    make_emit_fn(env, "term/show-cursor", "\x1b[?25h");
    make_emit_fn(env, "term/save-cursor", "\x1b7");
    make_emit_fn(env, "term/restore-cursor", "\x1b8");

    // Mouse reporting: button events (1000) + button-motion/drag (1002) + SGR
    // extended coords (1006). io/read-key decodes the reports into {:kind :mouse …}.
    make_emit_fn(
        env,
        "term/enable-mouse",
        "\x1b[?1000h\x1b[?1002h\x1b[?1006h",
    );
    make_emit_fn(
        env,
        "term/disable-mouse",
        "\x1b[?1000l\x1b[?1002l\x1b[?1006l",
    );

    // Kitty keyboard protocol (opt-in): push flags 17 = disambiguate (1) +
    // report-associated-text (16). No event-types flag, so no repeat/release
    // events (which would double-fire as key presses). Terminals without kitty
    // support silently ignore this, and keys keep coming through the legacy path.
    // io/read-key decodes `ESC [ … u` events, normalizing to the usual key maps
    // plus an optional :mods list. Restore with the stack pop on exit.
    make_emit_fn(env, "term/enable-kitty-keys!", "\x1b[>17u");
    make_emit_fn(env, "term/disable-kitty-keys!", "\x1b[<u");

    make_emit_fn(env, "term/bell", "\x07");

    register_fn(env, "term/move-to", |args| {
        check_arity!(args, "term/move-to", 2);
        let row = coord(&args[0])?;
        let col = coord(&args[1])?;
        emit(&format!("\x1b[{row};{col}H"))
    });

    register_fn(env, "term/write-at", |args| {
        check_arity!(args, "term/write-at", 3);
        let row = coord(&args[0])?;
        let col = coord(&args[1])?;
        let text = args[2]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
        emit(&format!("\x1b[{row};{col}H{text}"))
    });

    register_fn(env, "term/set-title", |args| {
        check_arity!(args, "term/set-title", 1);
        let title = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // OSC 0 sets both icon name and window title; BEL terminates.
        emit(&format!("\x1b]0;{title}\x07"))
    });

    // term/flush — explicit flush for code that batches styled writes with
    // io/print before showing a frame. (Control fns above already self-flush.)
    register_fn(env, "term/flush", |args| {
        check_arity!(args, "term/flush", 0);
        std::io::stdout()
            .flush()
            .map_err(|e| SemaError::Io(format!("term/flush: {e}")))?;
        Ok(Value::nil())
    });
}
