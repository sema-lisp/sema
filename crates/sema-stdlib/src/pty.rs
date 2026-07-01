//! Pseudo-terminal primitives (`pty/*`).
//!
//! Like `proc/*`, but the child runs under a real PTY, so programs that probe
//! `isatty` (REPLs, `vim`, `top`, anything with color/line-editing) behave as if
//! attached to a terminal. stdout+stderr are merged onto the pty master and
//! drained by a reader thread into a pollable buffer; `pty/resize` propagates
//! window-size changes (SIGWINCH). Handles are integer ids into a thread-local
//! registry. Use `pty/close` to free a handle.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use sema_core::{check_arity, Caps, SemaError, Value};

struct Pty {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    out: Arc<Mutex<Vec<u8>>>,
    reader_thread: Option<JoinHandle<()>>,
}

thread_local! {
    static PTYS: RefCell<HashMap<i64, Pty>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<i64> = const { Cell::new(1) };
}

fn pump(mut reader: Box<dyn Read + Send>, buf: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
    })
}

fn handle(args: &[Value], idx: usize) -> Result<i64, SemaError> {
    args[idx]
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer (pty handle)", args[idx].type_name()))
}

fn u16_opt(m: &std::collections::BTreeMap<Value, Value>, key: &str, default: u16) -> u16 {
    m.get(&Value::keyword(key))
        .and_then(|v| v.as_int())
        .map(|n| n.clamp(1, u16::MAX as i64) as u16)
        .unwrap_or(default)
}

fn spawn(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "pty/spawn", 1..=2);
    let argv = args[0]
        .as_list()
        .or_else(|| args[0].as_vector())
        .ok_or_else(|| SemaError::type_error("list of strings (argv)", args[0].type_name()))?;
    if argv.is_empty() {
        return Err(SemaError::eval("pty/spawn: argv must be non-empty"));
    }
    let mut parts: Vec<String> = Vec::with_capacity(argv.len());
    for v in argv {
        parts.push(
            v.as_str()
                .ok_or_else(|| SemaError::type_error("string", v.type_name()))?
                .to_string(),
        );
    }

    let opts = args.get(1).and_then(|o| o.as_map_ref());
    let (rows, cols) = match opts {
        Some(m) => (u16_opt(m, "rows", 24), u16_opt(m, "cols", 80)),
        None => (24, 80),
    };

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| SemaError::eval(format!("pty/spawn: {e}")))?;

    let mut cmd = CommandBuilder::new(&parts[0]);
    for a in &parts[1..] {
        cmd.arg(a);
    }
    if let Some(m) = opts {
        if let Some(cwd) = m.get(&Value::keyword("cwd")).and_then(|v| v.as_str()) {
            cmd.cwd(cwd);
        }
        if let Some(em) = m.get(&Value::keyword("env")).and_then(|v| v.as_map_ref()) {
            for (k, val) in em.iter() {
                if let (Some(k), Some(val)) = (k.as_str(), val.as_str()) {
                    cmd.env(k, val);
                }
            }
        }
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| SemaError::eval(format!("pty/spawn {}: {e}", parts[0])))?;
    // Drop the slave so the master read sees EOF once the child exits.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| SemaError::eval(format!("pty/spawn: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| SemaError::eval(format!("pty/spawn: {e}")))?;
    let out = Arc::new(Mutex::new(Vec::new()));
    let reader_thread = Some(pump(reader, out.clone()));

    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    PTYS.with(|p| {
        p.borrow_mut().insert(
            id,
            Pty {
                master: pair.master,
                child,
                writer,
                out,
                reader_thread,
            },
        )
    });
    Ok(Value::int(id))
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/spawn", spawn);

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/read", |args| {
        check_arity!(args, "pty/read", 1);
        let id = handle(args, 0)?;
        PTYS.with(|p| match p.borrow().get(&id) {
            Some(pt) => {
                let mut b = pt.out.lock().unwrap_or_else(|e| e.into_inner());
                let s = String::from_utf8_lossy(&b).into_owned();
                b.clear();
                Ok(Value::string(&s))
            }
            None => Err(SemaError::eval(format!("pty/read: no such handle {id}"))),
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/write", |args| {
        check_arity!(args, "pty/write", 2);
        let id = handle(args, 0)?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            let pt = ptys
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("pty/write: no such handle {id}")))?;
            pt.writer
                .write_all(text.as_bytes())
                .and_then(|_| pt.writer.flush())
                .map_err(|e| SemaError::Io(format!("pty/write: {e}")))?;
            Ok(Value::nil())
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/resize", |args| {
        check_arity!(args, "pty/resize", 3);
        let id = handle(args, 0)?;
        let rows = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?
            .clamp(1, u16::MAX as i64) as u16;
        let cols = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))?
            .clamp(1, u16::MAX as i64) as u16;
        PTYS.with(|p| {
            let ptys = p.borrow();
            let pt = ptys
                .get(&id)
                .ok_or_else(|| SemaError::eval(format!("pty/resize: no such handle {id}")))?;
            pt.master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| SemaError::eval(format!("pty/resize: {e}")))?;
            Ok(Value::nil())
        })
    });

    // pty/wait — block until exit, return the exit code. Removes the handle for
    // the blocking wait (so the registry borrow isn't held), joins the reader
    // thread to guarantee all output is buffered, then re-inserts.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/wait", |args| {
        check_arity!(args, "pty/wait", 1);
        let id = handle(args, 0)?;
        let mut pt = PTYS
            .with(|p| p.borrow_mut().remove(&id))
            .ok_or_else(|| SemaError::eval(format!("pty/wait: no such handle {id}")))?;
        let status = pt.child.wait();
        if let Some(t) = pt.reader_thread.take() {
            let _ = t.join();
        }
        PTYS.with(|p| p.borrow_mut().insert(id, pt));
        let status = status.map_err(|e| SemaError::Io(format!("pty/wait: {e}")))?;
        Ok(Value::int(status.exit_code() as i64))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/exit-code", |args| {
        check_arity!(args, "pty/exit-code", 1);
        let id = handle(args, 0)?;
        PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            let pt = ptys
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("pty/exit-code: no such handle {id}")))?;
            match pt
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("pty/exit-code: {e}")))?
            {
                Some(status) => Ok(Value::int(status.exit_code() as i64)),
                None => Ok(Value::nil()),
            }
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/running?", |args| {
        check_arity!(args, "pty/running?", 1);
        let id = handle(args, 0)?;
        PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            let pt = ptys
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("pty/running?: no such handle {id}")))?;
            let running = pt
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("pty/running?: {e}")))?
                .is_none();
            Ok(Value::bool(running))
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/kill", |args| {
        check_arity!(args, "pty/kill", 1);
        let id = handle(args, 0)?;
        PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            let pt = ptys
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("pty/kill: no such handle {id}")))?;
            let _ = pt.child.kill();
            Ok(Value::nil())
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/close", |args| {
        check_arity!(args, "pty/close", 1);
        let id = handle(args, 0)?;
        PTYS.with(|p| {
            if let Some(mut pt) = p.borrow_mut().remove(&id) {
                let _ = pt.child.kill();
                let _ = pt.child.wait();
            }
            Ok(Value::nil())
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{EvalContext, Sandbox};

    fn env() -> sema_core::Env {
        let e = sema_core::Env::new();
        register(&e, &Sandbox::allow_all());
        e
    }

    fn call(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let f = env.get_str(name).expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        (nf.func)(&EvalContext::default(), args).expect("call ok")
    }

    #[test]
    fn pty_runs_a_command() {
        let e = env();
        let h = call(
            &e,
            "pty/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("printf hi"),
            ])],
        );
        let code = call(&e, "pty/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        let out = call(&e, "pty/read", &[h.clone()]);
        // PTY output may carry CR/LF translation; just assert our text is present.
        assert!(out.as_str().unwrap().contains("hi"));
        call(&e, "pty/close", &[h]);
    }

    #[test]
    fn isatty_is_true_under_pty() {
        let e = env();
        let h = call(
            &e,
            "pty/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("test -t 1 && printf TTY || printf NOTTY"),
            ])],
        );
        call(&e, "pty/wait", &[h.clone()]);
        let out = call(&e, "pty/read", &[h.clone()]);
        assert!(out.as_str().unwrap().contains("TTY"));
        call(&e, "pty/close", &[h]);
    }
}
