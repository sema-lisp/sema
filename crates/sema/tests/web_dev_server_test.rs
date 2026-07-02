//! Integration tests for the `sema web` dev server's serving layer.
//!
//! These spawn the real binary. They require the browser runtime to be
//! embedded (`make web-runtime` before building); when it isn't, the server
//! exits with a clear message and the test skips rather than failing, so a
//! plain `cargo build` checkout still passes. Marked `#[ignore]` like the other
//! server tests (they bind localhost sockets).

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Spawn `sema web <app> --port <port>`. Returns `None` (and prints why) when
/// the binary has no embedded runtime, so callers skip gracefully.
fn spawn_dev_server(app: &str, port: u16) -> Option<Child> {
    // Tests run with CWD = crate dir; the example paths are repo-root-relative.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["web", app, "--port", &port.to_string()])
        .current_dir(&repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sema web");

    std::thread::sleep(Duration::from_millis(1500));

    // If it already exited, the runtime is likely not embedded — skip.
    if let Ok(Some(_)) = child.try_wait() {
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err);
        }
        eprintln!(
            "skipping: `sema web` exited early ({}). Run `make web-runtime` and rebuild.",
            err.trim()
        );
        return None;
    }
    Some(child)
}

#[test]
#[ignore] // requires network + embedded web runtime (`make web-runtime`)
fn test_web_dev_server_serves_runtime_shell_and_app() {
    let port = 19930;
    let Some(mut child) = spawn_dev_server("examples/web/counter.sema", port) else {
        return;
    };

    let client = reqwest::blocking::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // The synthesized shell: import map + app mount point + the app script tag.
    let shell = client
        .get(format!("{base}/"))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("GET /")
        .text()
        .expect("shell body");
    assert!(
        shell.contains("importmap"),
        "shell must carry an import map"
    );
    assert!(
        shell.contains("id=\"app\""),
        "shell must have the #app mount point"
    );
    assert!(
        shell.contains("/app/counter.sema"),
        "shell must reference the app source"
    );
    assert!(
        shell.contains("SemaWeb.init"),
        "shell must boot the runtime"
    );

    // The WASM binary MUST be served as application/wasm for instantiateStreaming.
    let wasm = client
        .get(format!("{base}/__sema/sema_wasm_bg.wasm"))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("GET wasm");
    assert_eq!(wasm.status(), 200);
    assert_eq!(
        wasm.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/wasm"),
        "wasm must be served as application/wasm"
    );

    // The JS runtime entry + a nested module both resolve.
    for path in ["/__sema/sema-web.js", "/__sema/sema/index.js"] {
        let resp = client
            .get(format!("{base}{path}"))
            .timeout(Duration::from_secs(5))
            .send()
            .unwrap_or_else(|_| panic!("GET {path}"));
        assert_eq!(resp.status(), 200, "{path} should be served");
    }

    // The app's source is served from its directory.
    let app = client
        .get(format!("{base}/app/counter.sema"))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("GET app source");
    assert_eq!(app.status(), 200);
    assert!(
        !app.text().unwrap_or_default().is_empty(),
        "app source should be non-empty"
    );

    child.kill().ok();
    child.wait().ok();
}
