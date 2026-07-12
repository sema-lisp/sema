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
        .map_err(|e| format!("web config injection failed: {}", e.inner()))?;
    // Configure LLM providers from env keys (as the CLI does) so the proxy can
    // reach real providers. Harmless when no keys are set.
    if llm {
        let _ = interp.eval_str("(llm/auto-configure)");
    }

    // `(__web/prepare)` decides how the browser loads the app and (re)builds the
    // multi-file archive. Registered natively because it reuses the compiler /
    // import tracer; the Sema server calls it at startup and on each reload.
    {
        use sema_core::{intern, NativeFn, SemaError, Value};
        let entry_pb = entry_canonical.clone();
        let build_pb = build_dir.clone();
        interp.global_env.set(
            intern("__web/prepare"),
            Value::native_fn(NativeFn::simple("__web/prepare", move |args| {
                if !args.is_empty() {
                    return Err(SemaError::arity("__web/prepare", "0", args.len()));
                }
                Ok(web_prepare(&entry_pb, &build_pb))
            })),
        );
    }

    interp
        .eval_str_in_global(include_str!("dev_server.sema"))
        .map_err(|e| format!("dev server error: {}", e.inner()))?;
    Ok(())
}

/// Decide how the browser should load the app and, for multi-file apps, build a
/// fresh `.vfs`. Single-file apps run from raw source (the browser compiles them
/// → uniform error overlay, no build step). Multi-file apps use `import`, which
/// can't resolve against the browser's (absent) filesystem, so they're compiled
/// to a `.vfs` archive under `build_dir` — the same artifact `sema build
/// --target web` produces, with correct import resolution. Returns a map
/// `{:mode "source"|"archive"|"error" :error? "..."}`. Called on startup and on
/// every reload, so adding/removing imports mid-session is handled.
fn web_prepare(entry: &std::path::Path, build_dir: &std::path::Path) -> sema_core::Value {
    let imports = match crate::import_tracer::trace_imports(entry) {
        Ok(m) => m,
        Err(e) => return web_mode_map("error", Some(&format!("import tracing failed: {e}"))),
    };
    if imports.is_empty() {
        return web_mode_map("source", None);
    }
    match crate::build_web_archive(entry, &[]) {
        Ok((bytes, _)) => match std::fs::write(build_dir.join("app.vfs"), &bytes) {
            Ok(()) => web_mode_map("archive", None),
            Err(e) => web_mode_map("error", Some(&format!("writing archive: {e}"))),
        },
        Err(e) => web_mode_map("error", Some(&e)),
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
        let url = format!("http://{host}:{port}");
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
