//! Integration tests for the `sema web` dev server's serving layer.
//!
//! These spawn the real binary and exercise its embedded browser runtime.
//! Marked `#[ignore]` like the other server tests because they bind localhost
//! sockets.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Spawn `sema web <app> --port <port> --no-open` and require it to remain up.
fn spawn_dev_server(app: &str, port: u16) -> Child {
    // Tests run with CWD = crate dir; the example paths are repo-root-relative.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["web", app, "--port", &port.to_string(), "--no-open"])
        .current_dir(&repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sema web");

    std::thread::sleep(Duration::from_millis(1500));

    if let Some(status) = child.try_wait().expect("query sema web process") {
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err);
        }
        panic!(
            "`sema web` exited during startup with {status}: {}",
            err.trim(),
        );
    }
    child
}

#[test]
#[ignore] // binds a localhost socket
fn test_web_dev_server_serves_runtime_shell_and_app() {
    let port = 19930;
    let mut child = spawn_dev_server("examples/web/counter.sema", port);

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
    // The LLM proxy is on by default, so the shell wires the browser's llm/*
    // at this origin.
    assert!(
        shell.contains("llmProxy"),
        "shell must wire the LLM proxy when enabled"
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

#[test]
#[ignore] // requires network + embedded runtime + ANTHROPIC_API_KEY
fn test_web_dev_server_llm_proxy_complete_live() {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("skipping: ANTHROPIC_API_KEY not set");
        return;
    }
    let port = 19931;
    let mut child = spawn_dev_server("examples/web/counter.sema", port);

    // The proxy speaks the production llm-proxy protocol: POST /complete with a
    // prompt returns {content}. Uses a cheap model.
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/complete"))
        .json(&serde_json::json!({
            "prompt": "Reply with exactly one word: hello",
            "model": "claude-haiku-4-5-20251001",
            "max-tokens": 10,
        }))
        .timeout(Duration::from_secs(30))
        .send()
        .expect("POST /complete");
    assert_eq!(resp.status(), 200, "proxy /complete should succeed");
    let body: serde_json::Value = resp.json().expect("json body");
    let content = body.get("content").and_then(|c| c.as_str()).unwrap_or("");
    assert!(
        !content.is_empty(),
        "response should carry content, got: {body}"
    );

    child.kill().ok();
    child.wait().ok();
}
