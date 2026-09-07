use crate::build_interpreter;
use crate::drain_async_scheduler;
use crate::print_cli_error;
use crate::print_error;
use sema_core::pretty_print;
use sema_core::SemaError;
use sema_eval::Interpreter;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub(crate) fn run_eval(
    use_stdin: bool,
    expr: Option<String>,
    json: bool,
    path: Option<String>,
    timeout_ms: u64,
    sandbox_arg: Option<String>,
    no_llm: bool,
) {
    // Get the program text
    let program = if use_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .unwrap_or_else(|e| {
                if json {
                    let msg = format!("could not read stdin: {e}");
                    print_eval_json(&EvalJsonResult::early_error(&msg));
                } else {
                    print_cli_error(format!("could not read stdin: {e}"));
                }
                std::process::exit(1);
            });
        buf
    } else if let Some(e) = expr {
        e
    } else {
        if json {
            print_eval_json(&EvalJsonResult::early_error(
                "Either --stdin or --expr is required",
            ));
        } else {
            print_cli_error("either --stdin or --expr is required");
        }
        std::process::exit(1);
    };

    // Set up sandbox
    let sandbox = match &sandbox_arg {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            if json {
                let msg = format!("Invalid sandbox: {e}");
                print_eval_json(&EvalJsonResult::early_error(&msg));
            } else {
                print_cli_error(e);
            }
            std::process::exit(1);
        }),
        None => sema_core::Sandbox::allow_all(),
    };

    let interpreter = build_interpreter(&sandbox);

    // Auto-configure LLM unless --no-llm
    if !no_llm {
        let _ = interpreter.eval_str("(llm/auto-configure)");
    }

    // Set file path for import resolution
    if let Some(ref p) = path {
        let file_path = std::path::Path::new(p);
        // Try to canonicalize; fall back to the raw path (supports unsaved/virtual buffers)
        let resolved = file_path
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(p));
        interpreter.ctx.push_file_path(resolved);
    }

    // In JSON mode, capture stdout/stderr from user code by overriding IO functions
    // (same approach as sema-wasm). This prevents print/println/display from
    // corrupting the JSON envelope on real stdout.
    let captured_stdout: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let captured_stderr: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    if json {
        install_capturing_io(&interpreter, &captured_stdout, &captured_stderr);
    }

    // Arm the VM's wall-clock deadline so runaway loops abort with an eval
    // error instead of hanging the caller. Armed after (llm/auto-configure)
    // so setup time is not billed to the user's program; it stays armed
    // through the async drain so spawned tasks are bounded too. A saturating
    // deadline (checked_add → None) means "no timeout", same as 0.
    if timeout_ms > 0 {
        let deadline =
            std::time::Instant::now().checked_add(std::time::Duration::from_millis(timeout_ms));
        interpreter.ctx.set_eval_deadline(deadline);
    }

    let start = std::time::Instant::now();
    let result = interpreter.eval_str_compiled(&program);
    if result.is_ok() {
        drain_async_scheduler(&interpreter);
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let stdout_text = captured_stdout.borrow();
    let stderr_text = captured_stderr.borrow();

    match result {
        Ok(val) => {
            if json {
                let val_str = if val.is_nil() {
                    None
                } else {
                    Some(pretty_print(&val, 120))
                };
                print_eval_json(&EvalJsonResult {
                    ok: true,
                    value: val_str.as_deref(),
                    stdout: &stdout_text,
                    stderr: &stderr_text,
                    error_msg: None,
                    error_hint: None,
                    error_line: None,
                    error_col: None,
                    elapsed_ms,
                });
            } else if !val.is_nil() {
                println!("{}", pretty_print(&val, 120));
            }
        }
        Err(e) => {
            let inner = e.inner();
            let msg = e.user_message();
            let hint = e.hint().map(|s| s.to_string());
            // Extract line+col from Reader span or first stack trace frame
            let (line, col) = match inner {
                SemaError::Reader { span, .. } => (Some(span.line), Some(span.col)),
                _ => e
                    .stack_trace()
                    .and_then(|t| t.0.first())
                    .and_then(|f| f.span.as_ref())
                    .map(|s| (Some(s.line), Some(s.col)))
                    .unwrap_or((None, None)),
            };
            if json {
                print_eval_json(&EvalJsonResult {
                    ok: false,
                    value: None,
                    stdout: &stdout_text,
                    stderr: &stderr_text,
                    error_msg: Some(&msg),
                    error_hint: hint.as_deref(),
                    error_line: line,
                    error_col: col,
                    elapsed_ms,
                });
            } else {
                print_error(&e);
                std::process::exit(1);
            }
        }
    }
}

/// Override display/print/println/pprint/newline/print-error/println-error to write
/// to in-memory buffers instead of real stdout/stderr. This prevents user code output
/// from corrupting the JSON envelope in `sema eval --json` mode.
fn install_capturing_io(
    interpreter: &Interpreter,
    stdout_buf: &Rc<RefCell<String>>,
    stderr_buf: &Rc<RefCell<String>>,
) {
    use sema_core::{intern, NativeFn, Value};
    let env = &interpreter.global_env;

    // Helper: register a simple native fn that captures to a buffer
    macro_rules! capture_fn {
        ($name:expr, $buf:expr, $newline:expr, $raw:expr) => {{
            let buf = $buf.clone();
            env.set(
                intern($name),
                Value::native_fn(NativeFn::simple($name, move |args| {
                    let mut out = buf.borrow_mut();
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push(' ');
                        }
                        if $raw {
                            out.push_str(&format!("{arg}"));
                        } else {
                            match arg.as_str() {
                                Some(s) => out.push_str(s),
                                None => out.push_str(&format!("{arg}")),
                            }
                        }
                    }
                    if $newline {
                        out.push('\n');
                    }
                    Ok(Value::nil())
                })),
            );
        }};
    }

    // stdout-targeting functions
    capture_fn!("display", stdout_buf, false, false);
    capture_fn!("print", stdout_buf, false, true);
    capture_fn!("println", stdout_buf, true, false);
    capture_fn!("newline", stdout_buf, true, false);

    // pprint needs special handling (uses pretty_print)
    let pprint_buf = stdout_buf.clone();
    env.set(
        intern("pprint"),
        Value::native_fn(NativeFn::simple("pprint", move |args| {
            sema_core::check_arity!(args, "pprint", 1);
            let mut out = pprint_buf.borrow_mut();
            out.push_str(&sema_core::pretty_print(&args[0], 80));
            out.push('\n');
            Ok(Value::nil())
        })),
    );

    // stderr-targeting functions
    capture_fn!("print-error", stderr_buf, false, false);
    capture_fn!("println-error", stderr_buf, true, false);
}

struct EvalJsonResult<'a> {
    ok: bool,
    value: Option<&'a str>,
    stdout: &'a str,
    stderr: &'a str,
    error_msg: Option<&'a str>,
    error_hint: Option<&'a str>,
    error_line: Option<usize>,
    error_col: Option<usize>,
    elapsed_ms: u64,
}

impl<'a> EvalJsonResult<'a> {
    /// An early-failure envelope used before evaluation runs: `ok:false`, no
    /// value, no captured output, no source span, zero elapsed time. The only
    /// thing that varies between sites is the error message.
    fn early_error(msg: &'a str) -> Self {
        Self {
            ok: false,
            value: None,
            stdout: "",
            stderr: "",
            error_msg: Some(msg),
            error_hint: None,
            error_line: None,
            error_col: None,
            elapsed_ms: 0,
        }
    }
}

fn print_eval_json(r: &EvalJsonResult) {
    let result = serde_json::json!({
        "ok": r.ok,
        "value": r.value,
        "stdout": r.stdout,
        "stderr": r.stderr,
        "error": r.error_msg.map(|msg| {
            let mut err = serde_json::json!({ "message": msg });
            if let Some(hint) = r.error_hint {
                err["hint"] = serde_json::json!(hint);
            }
            if let Some(line) = r.error_line {
                err["line"] = serde_json::json!(line);
            }
            if let Some(col) = r.error_col {
                err["col"] = serde_json::json!(col);
            }
            err
        }),
        "elapsedMs": r.elapsed_ms,
    });
    println!("{}", serde_json::to_string(&result).unwrap());
}
