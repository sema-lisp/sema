//! `sema web` — zero-config dev server for sema-web apps.
//!
//! The server logic lives in Sema (`dev_server.sema`, embedded via
//! `include_str!`); this launcher only extracts the embedded browser runtime,
//! hands the script its config, and runs it. The script blocks in `http/serve`,
//! so `run` blocks until the server is interrupted.

mod runtime;

use std::io::IsTerminal;

use sema_core::Sandbox;
use sema_eval::Interpreter;

/// Serve the sema-web app at `entry` in the browser. Blocks until interrupted.
pub fn run(entry: &str, host: &str, port: u16, open: bool, llm: bool) -> Result<(), String> {
    let entry_path = std::path::Path::new(entry);
    if !entry_path.is_file() {
        return Err(format!("app entry not found: {entry}"));
    }
    let entry_file = entry_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("invalid app entry: {entry}"))?
        .to_string();
    let app_dir = entry_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let app_dir = std::fs::canonicalize(&app_dir)
        .map_err(|e| format!("resolving app dir {}: {e}", app_dir.display()))?;
    let entry_canonical = std::fs::canonicalize(entry_path)
        .map_err(|e| format!("resolving app entry {entry}: {e}"))?;

    let runtime_dir = runtime::extract().map_err(|e| format!("extracting web runtime: {e}"))?;

    // Where the on-the-fly `.vfs` for a multi-file app is written (served under
    // /__build). Per-process so parallel dev servers don't collide.
    let build_dir = std::env::temp_dir().join(format!("sema-web-build-{}", std::process::id()));
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("creating build dir: {e}"))?;

    // Resolve a free port up front so auto-open targets the right URL. The probe
    // listener is dropped immediately; the Sema server rebinds it (a tiny race
    // window, fine for a local dev tool — it keeps :port-fallback on as a
    // backup).
    let (probe, actual_port) = sema_core::net::bind_with_fallback(host, port, 100)
        .map_err(|e| format!("no free port near {host}:{port}: {e}"))?;
    drop(probe);

    // The LLM proxy forwards to providers with the server's API keys and has
    // no authentication, so binding it to a reachable interface exposes those
    // keys to the network. Say so; `--no-llm` keeps the app reachable without it.
    if llm && !sema_core::net::is_loopback_host(host) {
        crate::print_cli_warning(format!(
            "--host {host} is not loopback: the unauthenticated LLM proxy is reachable from the network (pass --no-llm to disable it)"
        ));
    }

    // Auto-open only when attached to a terminal — never pop a browser from a
    // non-interactive run (CI, a pipe, or a test that forgot `--no-open`).
    if open && std::io::stdout().is_terminal() {
        spawn_browser_opener(host.to_string(), actual_port);
    }

    // Hand the Sema server its config as a double-encoded JSON string literal:
    // the inner JSON is what `dev_server.sema` decodes; the outer encoding makes
    // it a valid Sema string literal without hand-escaping paths.
    let config = serde_json::json!({
        "host": host,
        "port": actual_port,
        "entry": entry_file,
        "appDir": app_dir.to_string_lossy(),
        "runtimeDir": runtime_dir.to_string_lossy(),
        "buildDir": build_dir.to_string_lossy(),
        "open": open,
        "llm": llm,
        // The HTML shell template; dev_server.sema fills {{TITLE}}/{{APP}}/{{INIT}}.
        "shell": include_str!("shell.html"),
    });
    let config_literal = serde_json::to_string(&config.to_string())
        .map_err(|e| format!("encoding web config: {e}"))?;

    let sandbox = Sandbox::allow_all();
    let interp = Interpreter::new_with_sandbox(&sandbox);
    interp
        .eval_str_in_global(&format!("(define __web-config-json {config_literal})"))
        .map_err(|e| format!("web config injection failed: {}", e.format_plain()))?;
    // Configure LLM providers from env keys (as the CLI does) so the proxy can
    // reach real providers. Harmless when no keys are set.
    if llm {
        let _ = interp.eval_str("(llm/auto-configure)");
    }

    // `(__web/prepare)` decides how the browser loads the app and (re)builds the
    // multi-file archive. Registered natively because it reuses the compiler /
    // import tracer; the Sema server calls it at startup and on each reload.
    register_web_prepare(&interp, entry_canonical.clone(), build_dir.clone());

    interp
        .eval_str_in_global(include_str!("dev_server.sema"))
        .map_err(|e| format!("dev server failed: {}", e.format_plain()))?;
    Ok(())
}

/// Completion tag for a `__web/prepare` build job. Only needs to be consistent
/// between the issued identity and the prepared op; not a uniqueness key.
const WEB_PREPARE_COMPLETION_KIND: u64 = 0x7765_6270; // "webp"

/// Deadline net for a stuck build worker. A dev-server rebuild compiles a small
/// app; two minutes mirrors the stdlib's quarantined-compute cleanup deadline.
const WEB_PREPARE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// Register `__web/prepare` on `interp` as a dual-ABI native.
///
/// The dev server calls it from inside a runtime quantum (the top-level
/// `define` at startup and the `handle-poll` route on every reload). For a
/// multi-file app the build constructs fresh `Interpreter`s
/// (`compile_source_to_bytecode`), whose prelude load is a legacy VM entry —
/// forbidden while the thread-local quantum flag is set. So the runtime ABI
/// suspends on an external quarantined-blocking job: the build runs on an
/// executor worker thread (no quantum there), and the decoded `{:mode ...}`
/// map resumes the parked frame. The legacy ABI serves quantum-free callers
/// (tests, embedding) synchronously.
fn register_web_prepare(
    interp: &Interpreter,
    entry: std::path::PathBuf,
    build_dir: std::path::PathBuf,
) {
    use sema_core::runtime::{
        CompletionKind, NativeOutcome, NativeSuspend, PreparedExternalOperation, QuarantineBound,
        SendPayload, WaitKind,
    };
    use sema_core::{intern, NativeFn, SemaError, Value};

    let legacy_entry = entry.clone();
    let legacy_build = build_dir.clone();
    interp.global_env.set(
        intern("__web/prepare"),
        Value::native_fn(NativeFn::simple_with_runtime(
            "__web/prepare",
            move |args| {
                if !args.is_empty() {
                    return Err(SemaError::arity("__web/prepare", "0", args.len()));
                }
                let (mode, error) = web_prepare_send(&legacy_entry, &legacy_build);
                Ok(web_mode_map(&mode, error.as_deref()))
            },
            move |_context, args| {
                if !args.is_empty() {
                    return Err(SemaError::arity("__web/prepare", "0", args.len()));
                }
                let kind = CompletionKind::try_from_raw(WEB_PREPARE_COMPLETION_KIND)
                    .expect("web prepare completion kind is nonzero");
                let bound = QuarantineBound::hard_deadline(WEB_PREPARE_DEADLINE)
                    .expect("web prepare deadline is nonzero");
                let entry = entry.clone();
                let build_dir = build_dir.clone();
                let prepared = PreparedExternalOperation::quarantined_blocking(
                    kind,
                    Box::new(PrepareDecoder),
                    bound,
                    move || Ok(Box::new(web_prepare_send(&entry, &build_dir)) as SendPayload),
                );
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::External(Box::new(prepared)),
                    continuation: Box::new(PrepareContinuation),
                }))
            },
        )),
    );
}

/// Decodes the worker's `(mode, error)` payload into the `{:mode ...}` map on
/// the VM thread. Prepare never raises: any failure (including a worker panic)
/// becomes `{:mode "error" ...}` so `dev_server.sema`'s overlay shows it.
struct PrepareDecoder;

impl sema_core::runtime::Trace for PrepareDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl sema_core::runtime::CompletionDecoder for PrepareDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        use sema_core::runtime::downcast_send_payload;
        match result {
            Ok(payload) => {
                match downcast_send_payload::<(String, Option<String>)>(payload, "__web/prepare") {
                    Ok((mode, error)) => Ok(web_mode_map(&mode, error.as_deref())),
                    Err(failure) => Ok(web_mode_map("error", Some(failure.message()))),
                }
            }
            Err(failure) => Ok(web_mode_map("error", Some(failure.message()))),
        }
    }
}

/// Resumes the parked `__web/prepare` frame with the decoded map.
struct PrepareContinuation;

impl sema_core::runtime::Trace for PrepareContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl sema_core::runtime::NativeContinuation for PrepareContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput};
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(sema_core::SemaError::eval(format!(
                "__web/prepare was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(sema_core::SemaError::eval(
                "__web/prepare continuation received an unexpected runtime response",
            )),
        }
    }
}

/// Decide how the browser should load the app and, for multi-file apps, build a
/// fresh `.vfs`. Single-file apps run from raw source (the browser compiles them
/// → uniform error overlay, no build step). Multi-file apps use `import`, which
/// can't resolve against the browser's (absent) filesystem, so they're compiled
/// to a `.vfs` archive under `build_dir` — the same artifact `sema build
/// --target web` produces, with correct import resolution. Returns
/// `(mode, error)` with mode `"source"|"archive"|"error"`. Called on startup and
/// on every reload, so adding/removing imports mid-session is handled.
///
/// `Send` by construction (paths and bytes only): for a multi-file app the
/// compile builds fresh `Interpreter`s, so this must run on a thread with no
/// active runtime quantum — the worker job in [`register_web_prepare`], never
/// the VM thread.
fn web_prepare_send(
    entry: &std::path::Path,
    build_dir: &std::path::Path,
) -> (String, Option<String>) {
    // Strict tracing: the browser has no filesystem to fall back to, so an
    // import that does not resolve now can never resolve at runtime. Report it
    // as a build error instead of letting the page fail with a puzzling
    // "operation not supported on this platform".
    let imports = match crate::import_tracer::trace_imports_strict(entry) {
        Ok(snapshot) => snapshot.files,
        Err(e) => {
            let e = e.replace(
                " cannot be resolved for approval binding",
                " does not exist; a browser app has no filesystem, so every import must resolve at build time",
            );
            return (
                "error".to_string(),
                Some(format!("import tracing failed: {e}")),
            );
        }
    };
    if imports.is_empty() {
        return ("source".to_string(), None);
    }
    match crate::build::build_web_archive(entry, &[], crate::build::BuildOutputOpts::default()) {
        Ok((bytes, _)) => match std::fs::write(build_dir.join("app.vfs"), &bytes) {
            Ok(()) => ("archive".to_string(), None),
            Err(e) => ("error".to_string(), Some(format!("writing archive: {e}"))),
        },
        Err(e) => ("error".to_string(), Some(e)),
    }
}

fn web_mode_map(mode: &str, error: Option<&str>) -> sema_core::Value {
    use sema_core::Value;
    let mut m = std::collections::BTreeMap::new();
    m.insert(Value::keyword("mode"), Value::string(mode));
    if let Some(e) = error {
        m.insert(Value::keyword("error"), Value::string(e));
    }
    Value::map(m)
}

/// Open the app in the default browser once the server accepts connections.
/// Runs on a background thread so it doesn't block the (blocking) server loop.
fn spawn_browser_opener(host: String, port: u16) {
    std::thread::spawn(move || {
        // Wait for the server to start accepting connections (up to ~10s).
        for _ in 0..100 {
            if std::net::TcpStream::connect((host.as_str(), port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let url = format!("http://{}", sema_core::net::format_host_port(&host, port));
        let _ = open_url(&url);
    });
}

/// Open `url` with the OS's default handler. Best-effort; errors are ignored.
fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("sema-web-prepare-{tag}-{nanos}"));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn prepare_mode(dir: &TempDir, entry: &str) -> (String, std::path::PathBuf) {
        let entry = dir.0.join(entry);
        let build_dir = dir.0.join("build");
        std::fs::create_dir_all(&build_dir).unwrap();
        let sandbox = Sandbox::allow_all();
        let interp = Interpreter::new_with_sandbox(&sandbox);
        register_web_prepare(&interp, entry, build_dir.clone());
        // Top-level eval runs on the unified runtime, so `__web/prepare` is
        // dispatched inside a runtime quantum — the exact path that used to
        // panic ("legacy native callback cannot re-enter a VM during an
        // active runtime quantum") when the build constructed fresh
        // interpreters on the VM thread.
        let mode = interp
            .eval_str_in_global("(:mode (__web/prepare))")
            .unwrap()
            .as_str()
            .expect("mode is a string")
            .to_string();
        (mode, build_dir)
    }

    #[test]
    fn web_prepare_builds_multi_file_archive_inside_a_runtime_quantum() {
        let dir = TempDir::new("multi");
        std::fs::write(
            dir.0.join("lib.sema"),
            "(module lib (export greet) (define (greet) \"hi\"))",
        )
        .unwrap();
        std::fs::write(dir.0.join("app.sema"), "(import \"lib.sema\")\n(greet)").unwrap();

        let (mode, build_dir) = prepare_mode(&dir, "app.sema");
        assert_eq!(mode, "archive");
        assert!(build_dir.join("app.vfs").is_file());
    }

    #[test]
    fn web_prepare_single_file_stays_source_mode() {
        let dir = TempDir::new("single");
        std::fs::write(dir.0.join("app.sema"), "(+ 1 2)").unwrap();

        let (mode, build_dir) = prepare_mode(&dir, "app.sema");
        assert_eq!(mode, "source");
        assert!(!build_dir.join("app.vfs").exists());
    }

    #[test]
    fn web_prepare_broken_import_reports_error_mode() {
        let dir = TempDir::new("broken");
        // The imported file exists (so the app is multi-file) but does not
        // parse, so the archive build fails → the overlay contract:
        // {:mode "error"} rather than a raised error.
        std::fs::write(dir.0.join("lib.sema"), "(define (broken").unwrap();
        std::fs::write(dir.0.join("app.sema"), "(import \"lib.sema\")").unwrap();

        let (mode, _) = prepare_mode(&dir, "app.sema");
        assert_eq!(mode, "error");
    }
}
