//! `sema web` — zero-config dev server for sema-web apps.
//!
//! The server logic lives in Sema (`dev_server.sema`, embedded via
//! `include_str!`); this launcher only extracts the embedded browser runtime,
//! hands the script its config, and runs it. The script blocks in `http/serve`,
//! so `run` blocks until the server is interrupted.

mod runtime;

use sema_core::Sandbox;
use sema_eval::Interpreter;

/// Serve the sema-web app at `entry` in the browser. Blocks until interrupted.
pub fn run(entry: &str, host: &str, port: u16, open: bool, llm: bool) -> Result<(), String> {
    if !runtime::is_available() {
        return Err("this `sema` build has no embedded web runtime.\n  \
             Run `make web-runtime` to vendor it, then rebuild the binary."
            .to_string());
    }

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

    let runtime_dir = runtime::extract().map_err(|e| format!("extracting web runtime: {e}"))?;

    // Resolve a free port up front so auto-open targets the right URL. The probe
    // listener is dropped immediately; the Sema server rebinds it (a tiny race
    // window, fine for a local dev tool — it keeps :port-fallback on as a
    // backup).
    let (probe, actual_port) = sema_core::net::bind_with_fallback(host, port, 100)
        .map_err(|e| format!("no free port near {host}:{port}: {e}"))?;
    drop(probe);

    if open {
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
        "open": open,
        "llm": llm,
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
    interp
        .eval_str_in_global(include_str!("dev_server.sema"))
        .map_err(|e| format!("dev server error: {}", e.inner()))?;
    Ok(())
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
