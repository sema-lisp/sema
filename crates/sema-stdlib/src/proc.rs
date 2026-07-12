//! Streaming subprocess primitives (`proc/*`).
//!
//! Unlike `shell`, which blocks and returns the full output only after the
//! command exits, these expose a *live* handle: stdout/stderr are drained by
//! background reader threads into buffers you poll with `proc/read-stdout` /
//! `proc/read-stderr`, so a TUI can show test output as it streams. The handle
//! is an integer id into a thread-local registry (the VM is single-threaded, so
//! the registry never crosses threads — only the pipe readers do).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use sema_core::{check_arity, Caps, SemaError, Value};

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
    out_thread: Option<JoinHandle<()>>,
    err_thread: Option<JoinHandle<()>>,
}

thread_local! {
    static PROCS: RefCell<HashMap<i64, Proc>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<i64> = const { Cell::new(1) };
}

/// Spawn a thread that drains `reader` into `buf` until EOF. The returned
/// handle is joined by `proc/wait`: a finished join means EOF was reached, so
/// every byte the child wrote is in `buf` (the tail-buffering guarantee).
fn pump<R: Read + Send + 'static>(mut reader: R, buf: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
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

/// Take the integer handle from `args[idx]`.
fn handle(args: &[Value], idx: usize) -> Result<i64, SemaError> {
    args[idx]
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer (proc handle)", args[idx].type_name()))
}

/// Drain a buffer's current contents as a lossy-UTF-8 string (clearing it).
fn drain(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
    let s = String::from_utf8_lossy(&b).into_owned();
    b.clear();
    s
}

/// Poll a process handle for `event/select`: `Some((has_buffered_output,
/// has_exited))`, or `None` if the handle is unknown. Drives the TUI's "show
/// test output as it streams, then react to exit" loop.
pub(crate) fn poll_ready(id: i64) -> Option<(bool, bool)> {
    PROCS.with(|p| {
        let mut procs = p.borrow_mut();
        let pr = procs.get_mut(&id)?;
        let has_out = pr.out.lock().map(|b| !b.is_empty()).unwrap_or(false)
            || pr.err.lock().map(|b| !b.is_empty()).unwrap_or(false);
        let exited = matches!(pr.child.try_wait(), Ok(Some(_)));
        Some((has_out, exited))
    })
}

fn spawn(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "proc/spawn", 1..=2);
    let argv = args[0]
        .as_list()
        .or_else(|| args[0].as_vector())
        .ok_or_else(|| SemaError::type_error("list of strings (argv)", args[0].type_name()))?;
    if argv.is_empty() {
        return Err(SemaError::eval("proc/spawn: argv must be non-empty"));
    }
    let mut parts: Vec<String> = Vec::with_capacity(argv.len());
    for v in argv {
        parts.push(
            v.as_str()
                .ok_or_else(|| SemaError::type_error("string", v.type_name()))?
                .to_string(),
        );
    }

    let mut cmd = Command::new(&parts[0]);
    cmd.args(&parts[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Optional opts map: {:cwd "path" :env {"KEY" "val" ...}}. Shared extraction
    // with `shell` so both APIs interpret the map identically.
    if let Some(m) = args.get(1).and_then(|v| v.as_map_ref()) {
        let (cwd, env_vars) = crate::system::command_opts(m);
        if let Some(dir) = &cwd {
            cmd.current_dir(dir);
        }
        for (k, val) in &env_vars {
            cmd.env(k, val);
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| SemaError::eval(format!("proc/spawn {}: {e}", parts[0])))?;

    let out = Arc::new(Mutex::new(Vec::new()));
    let err = Arc::new(Mutex::new(Vec::new()));
    let out_thread = child.stdout.take().map(|so| pump(so, out.clone()));
    let err_thread = child.stderr.take().map(|se| pump(se, err.clone()));
    let stdin = child.stdin.take();

    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    PROCS.with(|p| {
        p.borrow_mut().insert(
            id,
            Proc {
                child,
                stdin,
                out,
                err,
                out_thread,
                err_thread,
            },
        )
    });
    Ok(Value::int(id))
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/spawn", spawn);

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/read-stdout", |args| {
        check_arity!(args, "proc/read-stdout", 1);
        let id = handle(args, 0)?;
        PROCS.with(|p| match p.borrow().get(&id) {
            Some(pr) => Ok(Value::string(&drain(&pr.out))),
            None => Err(SemaError::eval(format!(
                "proc/read-stdout: no such handle {id}"
            ))),
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/read-stderr", |args| {
        check_arity!(args, "proc/read-stderr", 1);
        let id = handle(args, 0)?;
        PROCS.with(|p| match p.borrow().get(&id) {
            Some(pr) => Ok(Value::string(&drain(&pr.err))),
            None => Err(SemaError::eval(format!(
                "proc/read-stderr: no such handle {id}"
            ))),
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/write-stdin", |args| {
        check_arity!(args, "proc/write-stdin", 2);
        let id = handle(args, 0)?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            let pr = procs
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("proc/write-stdin: no such handle {id}")))?;
            match pr.stdin.as_mut() {
                Some(sin) => {
                    sin.write_all(text.as_bytes())
                        .and_then(|_| sin.flush())
                        .map_err(|e| SemaError::Io(format!("proc/write-stdin: {e}")))?;
                    Ok(Value::nil())
                }
                None => Err(SemaError::eval("proc/write-stdin: stdin already closed")),
            }
        })
    });

    // proc/close-stdin — send EOF to the child by dropping its stdin.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/close-stdin", |args| {
        check_arity!(args, "proc/close-stdin", 1);
        let id = handle(args, 0)?;
        PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            let pr = procs
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("proc/close-stdin: no such handle {id}")))?;
            pr.stdin = None; // drop → EOF
            Ok(Value::nil())
        })
    });

    // proc/wait — block until exit, return the exit code (or -1 if signalled).
    // The handle is removed from the registry first so we don't hold the
    // thread-local borrow across the blocking wait (which would panic on any
    // reentrant registry access and would block other proc ops). Joining the
    // pump threads guarantees every byte the child wrote is buffered before we
    // return (so a following proc/read-stdout sees the tail). The proc is
    // re-inserted so reads still work after wait.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/wait", |args| {
        check_arity!(args, "proc/wait", 1);
        let id = handle(args, 0)?;
        let mut pr = PROCS
            .with(|p| p.borrow_mut().remove(&id))
            .ok_or_else(|| SemaError::eval(format!("proc/wait: no such handle {id}")))?;
        let status = pr.child.wait();
        if let Some(t) = pr.out_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = pr.err_thread.take() {
            let _ = t.join();
        }
        PROCS.with(|p| p.borrow_mut().insert(id, pr));
        let status = status.map_err(|e| SemaError::Io(format!("proc/wait: {e}")))?;
        Ok(Value::int(status.code().unwrap_or(-1) as i64))
    });

    // proc/exit-code — Some(code) if exited, nil if still running.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/exit-code", |args| {
        check_arity!(args, "proc/exit-code", 1);
        let id = handle(args, 0)?;
        PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            let pr = procs
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("proc/exit-code: no such handle {id}")))?;
            match pr
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("proc/exit-code: {e}")))?
            {
                Some(status) => Ok(Value::int(status.code().unwrap_or(-1) as i64)),
                None => Ok(Value::nil()),
            }
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/running?", |args| {
        check_arity!(args, "proc/running?", 1);
        let id = handle(args, 0)?;
        PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            let pr = procs
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("proc/running?: no such handle {id}")))?;
            let running = pr
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("proc/running?: {e}")))?
                .is_none();
            Ok(Value::bool(running))
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/kill", |args| {
        check_arity!(args, "proc/kill", 1);
        let id = handle(args, 0)?;
        PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            let pr = procs
                .get_mut(&id)
                .ok_or_else(|| SemaError::eval(format!("proc/kill: no such handle {id}")))?;
            let _ = pr.child.kill(); // ignore "already exited"
            Ok(Value::nil())
        })
    });

    // proc/close — kill if needed and drop the handle (frees the registry slot).
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/close", |args| {
        check_arity!(args, "proc/close", 1);
        let id = handle(args, 0)?;
        PROCS.with(|p| {
            if let Some(mut pr) = p.borrow_mut().remove(&id) {
                let _ = pr.child.kill();
                let _ = pr.child.wait();
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
    fn spawn_read_wait_roundtrip() {
        let e = env();
        let h = call(
            &e,
            "proc/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("printf hello; printf oops 1>&2"),
            ])],
        );
        let code = call(&e, "proc/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        assert_eq!(
            call(&e, "proc/read-stdout", &[h.clone()]).as_str(),
            Some("hello")
        );
        assert_eq!(
            call(&e, "proc/read-stderr", &[h.clone()]).as_str(),
            Some("oops")
        );
        call(&e, "proc/close", &[h]);
    }

    #[test]
    fn write_stdin_echoes() {
        let e = env();
        let h = call(&e, "proc/spawn", &[Value::list(vec![Value::string("cat")])]);
        call(
            &e,
            "proc/write-stdin",
            &[h.clone(), Value::string("ping\n")],
        );
        call(&e, "proc/close-stdin", &[h.clone()]);
        let code = call(&e, "proc/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        assert_eq!(
            call(&e, "proc/read-stdout", &[h.clone()]).as_str(),
            Some("ping\n")
        );
        call(&e, "proc/close", &[h]);
    }
}
