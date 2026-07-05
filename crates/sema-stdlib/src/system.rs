use std::cell::RefCell;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use sema_core::{check_arity, Caps, SemaError, Value};

use crate::register_fn;

/// Monotonic clock captured the first time it is needed — forced during
/// `register()` so it reflects interpreter/process startup, not the first call
/// to `sys/elapsed`. `sys/elapsed` reports nanoseconds since this instant.
fn process_start() -> std::time::Instant {
    use std::sync::OnceLock;
    static PROCESS_START: OnceLock<std::time::Instant> = OnceLock::new();
    *PROCESS_START.get_or_init(std::time::Instant::now)
}

/// Whether a PATH candidate is actually runnable. On Unix this requires an
/// execute permission bit, matching POSIX `which`; reporting a non-executable
/// file as a found command is wrong. On other platforms runnability is governed
/// by extension rules, so existence (already checked by the caller) suffices.
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

// ─── Signal pending flags (set by async signal handlers) ────────────────────
static SIGWINCH_PENDING: AtomicBool = AtomicBool::new(false);
static SIGINT_PENDING: AtomicBool = AtomicBool::new(false);
static SIGTERM_PENDING: AtomicBool = AtomicBool::new(false);

// ─── Signal callbacks (thread-local, keyed by signal number) ────────────────
// Values are Sema callables stored per-signal.
thread_local! {
    static SIGNAL_CALLBACKS: RefCell<HashMap<i32, Vec<Value>>> = RefCell::new(HashMap::new());
}

// ─── Signal handlers: only allowed to use async-signal-safe operations ───────
#[cfg(unix)]
extern "C" fn handle_sigwinch(_: libc::c_int) {
    SIGWINCH_PENDING.store(true, Ordering::Relaxed);
}

#[cfg(unix)]
extern "C" fn handle_sigint(_: libc::c_int) {
    SIGINT_PENDING.store(true, Ordering::Relaxed);
}

#[cfg(unix)]
extern "C" fn handle_sigterm(_: libc::c_int) {
    SIGTERM_PENDING.store(true, Ordering::Relaxed);
}

/// Resolve the (program, argv) for a `shell` invocation. A lone command string
/// runs through the system shell (`sh -c "<cmd>"` / `cmd /C "<cmd>"`); explicit
/// args run the program directly. Owned `String`s so the spawned async path can
/// move them across the thread boundary. Shared by both paths so they launch
/// byte-identical commands.
fn shell_program_args(cmd: &str, cmd_args: &[&str]) -> (String, Vec<String>) {
    if cmd_args.is_empty() {
        // Single string: run through the system shell for command parsing
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let flag = if cfg!(windows) { "/C" } else { "-c" };
        (shell.to_string(), vec![flag.to_string(), cmd.to_string()])
    } else {
        // Explicit args: run the command directly
        (
            cmd.to_string(),
            cmd_args.iter().map(|s| s.to_string()).collect(),
        )
    }
}

/// Decode a finished child's status/stdout/stderr into the Sema `shell` result
/// map. Identical shape for the sync and async paths: `:stdout`/`:stderr` are
/// lossy-UTF-8 strings and `:exit-code` is the exit code (or `-1` when the
/// process was terminated by a signal / has no code).
fn shell_output_value(status_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Value {
    let stdout = String::from_utf8_lossy(stdout).to_string();
    let stderr = String::from_utf8_lossy(stderr).to_string();

    let mut result = std::collections::BTreeMap::new();
    result.insert(Value::keyword("stdout"), Value::string(&stdout));
    result.insert(Value::keyword("stderr"), Value::string(&stderr));
    result.insert(
        Value::keyword("exit-code"),
        Value::int(status_code.unwrap_or(-1) as i64),
    );
    Value::map(result)
}

/// The subprocess facts that cross the thread boundary back from the I/O pool
/// to the VM thread. Only plain `Send` data — never a `Value`/`Rc`.
/// Decoded into the same `Value` shape as the sync path via [`shell_output_value`].
struct RawShellOutput {
    status_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// The offloaded (async-context) path: `io_spawn` the subprocess on the process-
/// wide I/O pool and yield an `AwaitIo` handle whose poll closure decodes the `Send`
/// output facts into the identical `Value` shape the sync path returns. Returns
/// `Ok(nil)` after arming the yield signal; the scheduler delivers the real
/// value on resume.
fn shell_async(program: String, child_args: Vec<String>) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<RawShellOutput, String>>();
    // The child's OS pid, published by the worker once spawned (0 = not yet). On Unix
    // the child is its OWN process-group leader (process_group(0) → pgid == pid), so
    // the abort hook can SIGKILL the whole group. The poll closure resets this to 0 on
    // completion so a later abort never signals a reaped (possibly reused) pid.
    let pid_slot = Arc::new(AtomicU32::new(0));
    let pid_for_worker = pid_slot.clone();
    let pid_for_poll = pid_slot.clone();

    let abort_task = sema_io::io_spawn(async move {
        let result = async {
            // Spawn (not `.output()`) so we can publish the pid before awaiting, then
            // gather output. `kill_on_drop` kills the direct child if this future is
            // dropped while the runtime is alive. stdout/stderr piped to match `.output()`.
            let mut cmd = tokio::process::Command::new(&program);
            cmd.args(&child_args)
                .kill_on_drop(true)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            // Put the child in its own process group so a compound/pipelined command
            // (`sh -c "a; b"`) — where `sh` forks the real workers as grandchildren —
            // can be torn down as a GROUP on abort, not just the `sh` leader.
            #[cfg(unix)]
            cmd.process_group(0);
            let child = cmd
                .spawn()
                // Match the sync path's spawn-error message format exactly.
                .map_err(|e| format!("shell: {e}"))?;
            if let Some(id) = child.id() {
                pid_for_worker.store(id, Ordering::SeqCst);
            }
            let output = child
                .wait_with_output()
                .await
                .map_err(|e| format!("shell: {e}"))?;
            Ok::<RawShellOutput, String>(RawShellOutput {
                status_code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
        .await;
        let _ = tx.send(result);
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });

    // True cancellation: on cancel/timeout the scheduler runs this hook. It (a) runs
    // the seam's one-shot AbortHook, aborting the spawned task (→ drops the
    // kill_on_drop `Child` once the pool processes it) AND (b) issues a SYNCHRONOUS
    // `SIGKILL` to the child's whole PROCESS GROUP. (b) is what makes the kill
    // reliable even when the program exits IMMEDIATELY after the timeout (e.g. a
    // one-shot `sema -e`), where the pool is torn down before it can process the
    // async abort — and killing the GROUP (not just the `sh` pid) reaps the
    // grandchildren a compound command forks. Fires only on cancellation; the killpg
    // layer composes AROUND the seam's hook.
    #[cfg(unix)]
    let pid_for_abort = pid_slot;
    #[cfg(not(unix))]
    let _ = pid_slot;
    let handle = Rc::new(sema_core::IoHandle::with_abort(
        move || match rx.try_recv() {
            Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
            Ok(Ok(raw)) => {
                pid_for_poll.store(0, Ordering::SeqCst);
                sema_core::IoPoll::Ready(Ok(shell_output_value(
                    raw.status_code,
                    &raw.stdout,
                    &raw.stderr,
                )))
            }
            Ok(Err(msg)) => {
                pid_for_poll.store(0, Ordering::SeqCst);
                sema_core::IoPoll::Ready(Err(msg))
            }
            Err(TryRecvError::Closed) => {
                pid_for_poll.store(0, Ordering::SeqCst);
                sema_core::IoPoll::Ready(Err("shell: subprocess worker dropped".to_string()))
            }
        },
        move || {
            abort_task();
            #[cfg(unix)]
            {
                let pid = pid_for_abort.load(Ordering::SeqCst);
                if pid != 0 {
                    // SAFETY: killpg of the child's own process group (process_group(0)
                    // set pgid == pid). The negative pid targets the GROUP, killing the
                    // `sh` leader AND any grandchildren it forked. The pid is reset to 0
                    // by the poll closure once the worker observed completion, so a
                    // reaped/reused pid is never targeted (only a constant signal is sent).
                    unsafe {
                        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                    }
                }
            }
        },
    ));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // Anchor the sys/elapsed clock at startup rather than at its first call.
    process_start();

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "env", |args| {
        check_arity!(args, "env", 1);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        match std::env::var(name) {
            Ok(val) => Ok(Value::string(&val)),
            Err(_) => Ok(Value::nil()),
        }
    });

    // shell requires both SHELL (to launch a shell) AND PROCESS (it spawns a child
    // process). Gate on SHELL via the helper, and check PROCESS inline so either
    // denial blocks the call.
    let shell_sandbox = sandbox.clone();
    crate::register_fn_gated(env, sandbox, Caps::SHELL, "shell", move |args| {
        shell_sandbox.check(Caps::PROCESS, "shell")?;
        check_arity!(args, "shell", 1..);
        let cmd = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let cmd_args: Vec<&str> = args[1..]
            .iter()
            .map(|a| {
                a.as_str()
                    .ok_or_else(|| SemaError::type_error("string", a.type_name()))
            })
            .collect::<Result<_, _>>()?;

        // Resolve the program + argv exactly once, shared by both paths so they
        // launch byte-identical commands.
        let (program, child_args) = shell_program_args(cmd, &cmd_args);

        // Inside an `async/spawn`'d task: offload the subprocess onto the
        // process-wide I/O pool and yield `AwaitIo` so the scheduler can run
        // sibling tasks while the child runs. Args are resolved and the result
        // `Value` decoded on the VM thread; only `Send` facts cross the boundary.
        if sema_core::in_async_context() {
            return shell_async(program, child_args);
        }

        // Top-level (not in a scheduler task): the original synchronous path,
        // byte-identical in observable behavior to the pre-async implementation.
        let output = std::process::Command::new(&program)
            .args(&child_args)
            .output()
            .map_err(|e| SemaError::Io(format!("shell: {e}")))?;

        Ok(shell_output_value(
            output.status.code(),
            &output.stdout,
            &output.stderr,
        ))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "exit", |args| {
        let code = if args.is_empty() {
            0
        } else {
            // Reject non-numeric status rather than silently exiting 0 — a script
            // that means to fail must not succeed. Floats truncate toward zero.
            match args[0]
                .as_int()
                .or_else(|| args[0].as_float().map(|f| f as i64))
            {
                Some(n) => n as i32,
                None => return Err(SemaError::type_error("integer", args[0].type_name())),
            }
        };
        std::process::exit(code);
    });

    fn time_ms_impl(args: &[Value]) -> Result<Value, SemaError> {
        check_arity!(args, "time-ms", 0);
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Ok(Value::int(ms))
    }
    register_fn(env, "time-ms", time_ms_impl);
    // Canonical slash-namespaced alias (Decision #24)
    register_fn(env, "time/now-ms", time_ms_impl);

    register_fn(env, "sleep", |args| {
        check_arity!(args, "sleep", 1);
        let ms = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
        Ok(Value::nil())
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "sys/args", |args| {
        check_arity!(args, "sys/args", 0);
        let args_list: Vec<Value> = std::env::args().map(|a| Value::string(&a)).collect();
        Ok(Value::list(args_list))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/cwd", |args| {
        check_arity!(args, "sys/cwd", 0);
        let cwd = std::env::current_dir().map_err(|e| SemaError::Io(format!("sys/cwd: {e}")))?;
        Ok(Value::string(&cwd.to_string_lossy()))
    });

    register_fn(env, "sys/platform", |args| {
        check_arity!(args, "sys/platform", 0);
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "unknown"
        };
        Ok(Value::string(platform))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_WRITE, "sys/set-env", |args| {
        check_arity!(args, "sys/set-env", 2);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let value = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        // set_var mutates process-global state and is UB under concurrent getenv
        // (tokio workers, C libs) on glibc. Serialize all sets behind a lock
        // (STD-12). NOTE: still unsafe if other code reads env concurrently —
        // sys/set-env should not be called while a server is handling requests.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var(name, value);
        }
        Ok(Value::nil())
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/env-all", |args| {
        check_arity!(args, "sys/env-all", 0);
        let mut map = std::collections::BTreeMap::new();
        for (key, val) in std::env::vars() {
            map.insert(Value::keyword(&key), Value::string(&val));
        }
        Ok(Value::map(map))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/home-dir", |args| {
        check_arity!(args, "sys/home-dir", 0);
        match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            Ok(home) => Ok(Value::string(&home)),
            Err(_) => Ok(Value::nil()),
        }
    });

    register_fn(env, "sys/sema-home", |args| {
        check_arity!(args, "sys/sema-home", 0);
        Ok(Value::string(&sema_core::sema_home().to_string_lossy()))
    });

    // sys/config-dir — platform-appropriate user config base directory, so apps
    // (e.g. Sema Coder) can store config without branching on OS in Sema:
    //   macOS:   ~/Library/Application Support
    //   Windows: %APPDATA%
    //   else:    $XDG_CONFIG_HOME or ~/.config
    register_fn(env, "sys/config-dir", |args| {
        check_arity!(args, "sys/config-dir", 0);
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        let dir = if cfg!(target_os = "macos") {
            std::path::PathBuf::from(&home)
                .join("Library")
                .join("Application Support")
        } else if cfg!(windows) {
            std::env::var("APPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::path::PathBuf::from(&home)
                        .join("AppData")
                        .join("Roaming")
                })
        } else {
            std::env::var("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from(&home).join(".config"))
        };
        Ok(Value::string(&dir.to_string_lossy()))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/temp-dir", |args| {
        check_arity!(args, "sys/temp-dir", 0);
        Ok(Value::string(&std::env::temp_dir().to_string_lossy()))
    });

    register_fn(env, "sys/hostname", |args| {
        check_arity!(args, "sys/hostname", 0);
        match hostname::get() {
            Ok(name) => Ok(Value::string(&name.to_string_lossy())),
            Err(_) => Ok(Value::nil()),
        }
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/user", |args| {
        check_arity!(args, "sys/user", 0);
        match std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
            Ok(user) => Ok(Value::string(&user)),
            Err(_) => Ok(Value::nil()),
        }
    });

    register_fn(env, "sys/interactive?", |args| {
        check_arity!(args, "sys/interactive?", 0);
        Ok(Value::bool(std::io::stdin().is_terminal()))
    });

    register_fn(env, "sys/tty", |args| {
        check_arity!(args, "sys/tty", 0);
        if !std::io::stdin().is_terminal() {
            return Ok(Value::nil());
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = std::io::stdin().as_raw_fd();
            unsafe {
                let name = libc::ttyname(fd);
                if name.is_null() {
                    Ok(Value::nil())
                } else {
                    let s = std::ffi::CStr::from_ptr(name).to_string_lossy().to_string();
                    Ok(Value::string(&s))
                }
            }
        }
        #[cfg(not(unix))]
        {
            Ok(Value::nil())
        }
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "sys/pid", |args| {
        check_arity!(args, "sys/pid", 0);
        Ok(Value::int(std::process::id() as i64))
    });

    register_fn(env, "sys/arch", |args| {
        check_arity!(args, "sys/arch", 0);
        Ok(Value::string(std::env::consts::ARCH))
    });

    register_fn(env, "sys/os", |args| {
        check_arity!(args, "sys/os", 0);
        Ok(Value::string(std::env::consts::OS))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "sys/which", |args| {
        check_arity!(args, "sys/which", 1);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let path_var = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ';' } else { ':' };
        // On Windows a bare name resolves by trying each PATHEXT suffix (unless
        // it already has an extension); on Unix the name is used verbatim.
        let candidates: Vec<String> =
            if cfg!(windows) && std::path::Path::new(name).extension().is_none() {
                let exts =
                    std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
                let mut names = vec![name.to_string()];
                names.extend(
                    exts.split(';')
                        .map(str::trim)
                        .filter(|e| !e.is_empty())
                        .map(|e| format!("{name}{e}")),
                );
                names
            } else {
                vec![name.to_string()]
            };
        for dir in path_var.split(sep) {
            for cand in &candidates {
                let candidate = std::path::Path::new(dir).join(cand);
                if candidate.is_file() && is_executable(&candidate) {
                    return Ok(Value::string(&candidate.to_string_lossy()));
                }
            }
        }
        Ok(Value::nil())
    });

    register_fn(env, "sys/interner-stats", |args| {
        check_arity!(args, "sys/interner-stats", 0);
        let (count, bytes) = sema_core::interner_stats();
        let mut result = std::collections::BTreeMap::new();
        result.insert(Value::keyword("count"), Value::int(count as i64));
        result.insert(Value::keyword("bytes"), Value::int(bytes as i64));
        Ok(Value::map(result))
    });

    register_fn(env, "sys/elapsed", |args| {
        check_arity!(args, "sys/elapsed", 0);
        let nanos = process_start().elapsed().as_nanos() as i64;
        Ok(Value::int(nanos))
    });

    // sys/term-size — returns {:rows N :cols M} or nil when not a TTY
    register_fn(env, "sys/term-size", |args| {
        check_arity!(args, "sys/term-size", 0);
        #[cfg(unix)]
        {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            // Try each standard fd in order until one succeeds; stdout is most reliable
            // for terminal size since stderr is used for status lines on many setups.
            for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO, libc::STDIN_FILENO] {
                let ret = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
                if ret == 0 && ws.ws_row > 0 && ws.ws_col > 0 {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(Value::keyword("rows"), Value::int(ws.ws_row as i64));
                    m.insert(Value::keyword("cols"), Value::int(ws.ws_col as i64));
                    return Ok(Value::map(m));
                }
            }
            Ok(Value::nil())
        }
        #[cfg(not(unix))]
        Ok(Value::nil())
    });

    // ─── Signal hooks (Unix only) ────────────────────────────────────────────
    // sys/on-signal — register a Sema callback for a signal.
    // Supported signals: :winch (SIGWINCH), :int (SIGINT), :term (SIGTERM).
    // Registering a handler installs the OS signal handler the first time.
    // Call (sys/check-signals) from your event loop to dispatch pending callbacks.
    #[cfg(unix)]
    {
        use sema_core::NativeFn;

        env.set(
            sema_core::intern("sys/on-signal"),
            Value::native_fn(NativeFn::with_ctx("sys/on-signal", |_ctx, args| {
                check_arity!(args, "sys/on-signal", 2);
                let kw = args[0]
                    .as_keyword()
                    .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
                let sig_num = match kw.as_str() {
                    "winch" => libc::SIGWINCH,
                    "int" => libc::SIGINT,
                    "term" => libc::SIGTERM,
                    other => {
                        return Err(SemaError::eval(format!(
                            "sys/on-signal: unknown signal :{other}; use :winch, :int, or :term"
                        )))
                    }
                };
                let callback = args[1].clone();
                // Install the OS-level signal handler on first registration
                SIGNAL_CALLBACKS.with(|cbs| {
                    let mut map = cbs.borrow_mut();
                    let entry = map.entry(sig_num).or_default();
                    if entry.is_empty() {
                        // First callback for this signal: install handler.
                        // Cast via *const () to avoid the fn_to_numeric_cast lint.
                        let handler: libc::sighandler_t = match sig_num {
                            s if s == libc::SIGWINCH => handle_sigwinch as *const () as usize,
                            s if s == libc::SIGINT => handle_sigint as *const () as usize,
                            s if s == libc::SIGTERM => handle_sigterm as *const () as usize,
                            // Unreachable: sig_num is validated against the three above by the
                            // kw match earlier in this function.
                            _ => unreachable!("unexpected signal number {sig_num}"),
                        };
                        unsafe { libc::signal(sig_num, handler) };
                    }
                    entry.push(callback);
                });
                Ok(Value::nil())
            })),
        );

        // sys/check-signals — call all pending signal callbacks.
        // Should be called from the main event loop (e.g., after io/read-key returns).
        env.set(
            sema_core::intern("sys/check-signals"),
            Value::native_fn(NativeFn::with_ctx("sys/check-signals", |ctx, args| {
                check_arity!(args, "sys/check-signals", 0);
                let mut to_dispatch: Vec<(i32, Vec<Value>)> = Vec::new();

                if SIGWINCH_PENDING.swap(false, Ordering::Relaxed) {
                    SIGNAL_CALLBACKS.with(|cbs| {
                        if let Some(callbacks) = cbs.borrow().get(&libc::SIGWINCH) {
                            to_dispatch.push((libc::SIGWINCH, callbacks.clone()));
                        }
                    });
                }
                if SIGINT_PENDING.swap(false, Ordering::Relaxed) {
                    SIGNAL_CALLBACKS.with(|cbs| {
                        if let Some(callbacks) = cbs.borrow().get(&libc::SIGINT) {
                            to_dispatch.push((libc::SIGINT, callbacks.clone()));
                        }
                    });
                }
                if SIGTERM_PENDING.swap(false, Ordering::Relaxed) {
                    SIGNAL_CALLBACKS.with(|cbs| {
                        if let Some(callbacks) = cbs.borrow().get(&libc::SIGTERM) {
                            to_dispatch.push((libc::SIGTERM, callbacks.clone()));
                        }
                    });
                }

                for (_, callbacks) in to_dispatch {
                    for cb in &callbacks {
                        sema_core::call_callback(ctx, cb, &[])?;
                    }
                }
                Ok(Value::nil())
            })),
        );
    }
}
