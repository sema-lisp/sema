//! `sema workflow view` — a tiny read-only web viewer over workflow run journals.
//!
//! Spike (scope doc `docs/plans/2026-06-23-workflow-dashboard-scope.md`, Option A):
//! a self-contained AlpineJS tree viewer that `fetch()`es a run's `events.jsonl` and
//! renders the Claude-Code-`/workflows`-style live tree, served by a minimal
//! loopback HTTP server. The richer Option B (SQLite projection + server-side
//! live-tail cursor) is a later upgrade; this spike parses the frozen journal
//! client-side and polls while a run is still `running`.
//!
//! Security: loopback-only by default and NO auth — the same trusted-local-developer
//! tool model the notebook server documents. Binding a non-loopback host exposes the
//! run directory's contents to the network; that is the operator's responsibility.
//!
//! No new crate dependency: a ~hand-rolled HTTP/1.1 handler over the `tokio` net/io
//! the binary already pulls in (the notebook uses axum; a 4-route read-only static
//! server does not need it).

use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub mod ingest;

const INDEX_HTML: &str = include_str!("workflow_view/index.html");
const ALPINE_JS: &str = include_str!("workflow_view/alpine.min.js");

/// Serve the viewer for the runs under `run_dir`, exiting the process on a bind
/// failure. Used by the standalone `sema workflow view` command.
pub async fn serve(run_dir: PathBuf, host: &str, port: u16) {
    if let Err(e) = serve_result(run_dir, host, port, true).await {
        eprintln!("sema workflow view: {e}");
        std::process::exit(1);
    }
}

/// Serve the viewer, returning a bind error instead of exiting — so an embedded
/// viewer (`workflow run --view`) can degrade to a warning without killing the run.
/// `announce` prints the URL on a successful bind.
pub async fn serve_result(
    run_dir: PathBuf,
    host: &str,
    port: u16,
    announce: bool,
) -> std::io::Result<()> {
    let addr = format!("{host}:{port}");
    let listener = TcpListener::bind(&addr).await?;
    if announce {
        println!("Sema workflow viewer:  http://{addr}");
        println!("  runs: {}", run_dir.display());
    }
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let run_dir = run_dir.clone();
                tokio::spawn(async move {
                    let _ = handle(stream, &run_dir).await;
                });
            }
            Err(e) => eprintln!("workflow view: accept error: {e}"),
        }
    }
}

/// Read one request, route it, write one response, close. (No keep-alive — fine for
/// a local dev viewer; the browser opens fresh connections per fetch.)
async fn handle(mut stream: TcpStream, run_dir: &Path) -> std::io::Result<()> {
    // Read until end of headers. Requests here are tiny (GET, no body).
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    // Strip any query string before routing.
    let path = path.split('?').next().unwrap_or("/");

    let (status, content_type, body): (&str, &str, Vec<u8>) = route(path, run_dir);
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

fn route(path: &str, run_dir: &Path) -> (&'static str, &'static str, Vec<u8>) {
    if path == "/" {
        return ("200 OK", "text/html; charset=utf-8", INDEX_HTML.into());
    }
    if path == "/alpine.min.js" {
        return (
            "200 OK",
            "application/javascript; charset=utf-8",
            ALPINE_JS.into(),
        );
    }
    if path == "/api/runs" {
        return (
            "200 OK",
            "application/json",
            list_runs(run_dir).into_bytes(),
        );
    }
    // Additive cross-run index (SQLite projection): a rich runs list with status, agent
    // count, tokens, and NULL-aware cost. Leaves the per-run JSONL routes untouched.
    if path == "/api/index/runs" {
        return ("200 OK", "application/json", index_runs_json(run_dir));
    }
    // /api/run/<id>/<file> — <id> a single safe segment, <file> a whitelisted name.
    if let Some(rest) = path.strip_prefix("/api/run/") {
        if let Some((id, file)) = rest.split_once('/') {
            let (ctype, ok) = match file {
                "events.jsonl" => ("application/x-ndjson", true),
                "result.json" | "metadata.json" | "args.json" => ("application/json", true),
                _ => ("", false),
            };
            if ok && is_safe_segment(id) {
                if let Ok(bytes) = std::fs::read(run_dir.join(id).join(file)) {
                    return ("200 OK", ctype, bytes);
                }
            }
        }
        return ("404 Not Found", "text/plain", b"no such run/file".to_vec());
    }
    ("404 Not Found", "text/plain", b"not found".to_vec())
}

/// JSON array of run-ids: immediate child dirs of `run_dir` that contain an
/// `events.jsonl`, newest first (by mtime).
fn list_runs(run_dir: &Path) -> String {
    let mut runs: Vec<(std::time::SystemTime, String)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(run_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let journal = p.join("events.jsonl");
            if journal.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    let mtime = journal
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    runs.push((mtime, name.to_string()));
                }
            }
        }
    }
    runs.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    let items: Vec<String> = runs
        .into_iter()
        .map(|(_, n)| format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", items.join(","))
}

/// The cross-run SQLite summary as a JSON array. Opens `<run-dir>/index.db` per request
/// (open-per-request is fine for a loopback dev tool — no shared-mutex concurrency),
/// lazily backfills every run, and serializes the rich summary. Degrades to `[]` on any
/// SQLite error so the endpoint never 500s the viewer.
fn index_runs_json(run_dir: &Path) -> Vec<u8> {
    let try_build = || -> rusqlite::Result<Vec<u8>> {
        let conn = ingest::open(&run_dir.join(sema_workflow::INDEX_DB))?;
        ingest::backfill_all(&conn, run_dir);
        let rows = ingest::runs_summary(&conn)?;
        Ok(serde_json::to_vec(&serde_json::Value::Array(rows)).unwrap_or_else(|_| b"[]".to_vec()))
    };
    try_build().unwrap_or_else(|_| b"[]".to_vec())
}

/// A run-id is a single directory segment: no separators, no `..`, non-empty.
fn is_safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains("..")
        && !s.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_segment_rejects_traversal() {
        assert!(is_safe_segment("content-live4"));
        assert!(is_safe_segment("wf_2026_123"));
        assert!(!is_safe_segment(""));
        assert!(!is_safe_segment(".."));
        assert!(!is_safe_segment("../etc"));
        assert!(!is_safe_segment("a/b"));
        assert!(!is_safe_segment("a\\b"));
        assert!(!is_safe_segment("x\0y"));
    }

    #[test]
    fn static_routes_resolve() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let (s1, ct1, b1) = route("/", dir);
        assert_eq!(s1, "200 OK");
        assert!(ct1.starts_with("text/html"));
        assert!(!b1.is_empty());

        let (s2, ct2, _) = route("/alpine.min.js", dir);
        assert_eq!(s2, "200 OK");
        assert!(ct2.contains("javascript"));

        // Unknown run dir → empty runs list, still valid JSON.
        let (s3, _, b3) = route("/api/runs", dir);
        assert_eq!(s3, "200 OK");
        assert_eq!(String::from_utf8(b3).unwrap(), "[]");

        // Traversal in the run id → 404, never reads outside run_dir.
        let (s4, _, _) = route("/api/run/../../etc/events.jsonl", dir);
        assert_eq!(s4, "404 Not Found");

        let (s5, _, _) = route("/nope", dir);
        assert_eq!(s5, "404 Not Found");
    }
}
