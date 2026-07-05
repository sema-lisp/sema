//! Pool-identity oracle for ADR #69 (one I/O pool behind one seam).
//!
//! Drives every offload kind — async http + shell (the `io_spawn` tier), async
//! `llm/complete` via FakeProvider (the `io_spawn_blocking` tier), sync
//! `llm/complete`, and sync `http/get` (the `io_block_on` tier) — then asserts
//! they were all served by THE one `sema-io` pool:
//!
//! - `sema_io::pools_built() == 1`: the pool builder ran exactly once for the
//!   whole battery (a second builder path sneaking into sema-io would trip it).
//! - `sema_io::block_on_ops()` advanced across the sync http call: the
//!   `block_on` tier drives futures ON THE CALLING THREAD (pool thread names
//!   can never observe it — empirical probe d), so the oracle counts seam
//!   entries instead.
//! - Direct `io_spawn` / `io_spawn_blocking` probes (closures we control)
//!   observe their work running on a `sema-io-*`-named thread — proving the
//!   spawn tiers actually land on the named pool.
//!
//! One test fn: the tiers share process-global counters, so a single ordered
//! drive keeps the deltas deterministic.

#![cfg(not(target_arch = "wasm32"))]

use std::net::TcpListener as StdTcpListener;
use std::sync::mpsc;
use std::time::Duration;

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

/// Minimal local HTTP fixture: accepts connections on a background std thread
/// pool-free (plain std, so the fixture itself cannot touch pool identity) and
/// answers every request with a fixed body after a tiny delay.
fn start_local_server() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().expect("local_addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut socket) = stream else { continue };
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf);
                std::thread::sleep(Duration::from_millis(10));
                let body = "pong";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes());
            });
        }
    });
    std::thread::sleep(Duration::from_millis(30));
    port
}

#[test]
fn one_pool_serves_every_offload_kind() {
    let port = start_local_server();

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .echo()
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // ── spawn tier probe (closure we control): thread name is sema-io-* ────
    let (tx, rx) = mpsc::channel::<String>();
    let _hook = sema_io::io_spawn(async move {
        let name = std::thread::current().name().unwrap_or("").to_string();
        let _ = tx.send(name);
    });
    let spawn_thread = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("io_spawn probe completed");
    assert!(
        spawn_thread.starts_with("sema-io-"),
        "io_spawn work must run on a sema-io-* thread, ran on {spawn_thread:?}"
    );

    // ── spawn_blocking tier probe: thread name is sema-io-* ────────────────
    let (tx, rx) = mpsc::channel::<String>();
    sema_io::io_spawn_blocking(move || {
        let name = std::thread::current().name().unwrap_or("").to_string();
        let _ = tx.send(name);
    });
    let blocking_thread = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("io_spawn_blocking probe completed");
    assert!(
        blocking_thread.starts_with("sema-io-"),
        "io_spawn_blocking work must run on a sema-io-* thread, ran on {blocking_thread:?}"
    );

    // ── async http (io_spawn tier through the stdlib offload) ──────────────
    let v = interp
        .eval_str_compiled(&format!(
            r#"(first (async/all (list (async/spawn
                 (fn () (:body (http/get "http://127.0.0.1:{port}/")))))))"#
        ))
        .expect("async http/get");
    assert_eq!(v.as_str(), Some("pong"));

    // ── async shell (io_spawn tier through the stdlib offload) ─────────────
    let v = interp
        .eval_str_compiled(
            r#"(first (async/all (list (async/spawn
                 (fn () (:exit-code (shell "true")))))))"#,
        )
        .expect("async shell");
    assert_eq!(v.as_int(), Some(0));

    // ── async llm/complete (io_spawn_blocking tier, FakeProvider) ──────────
    let v = interp
        .eval_str_compiled(
            r#"(first (async/all (list (async/spawn
                 (fn () (llm/complete "echoed"))))))"#,
        )
        .expect("async llm/complete");
    assert_eq!(v.as_str(), Some("echoed"));

    // ── sync llm/complete (VM thread; FakeProvider needs no wire call) ─────
    let v = interp
        .eval_str_compiled(r#"(llm/complete "sync-echo")"#)
        .expect("sync llm/complete");
    assert_eq!(v.as_str(), Some("sync-echo"));

    // ── sync http/get: the io_block_on tier. Thread names cannot observe
    //    block_on (it drives on the calling thread), so assert the seam
    //    counter advanced instead. ───────────────────────────────────────────
    let ops_before = sema_io::block_on_ops();
    let v = interp
        .eval_str_compiled(&format!(
            r#"(:status (http/get "http://127.0.0.1:{port}/"))"#
        ))
        .expect("sync http/get");
    assert_eq!(v.as_int(), Some(200));
    assert!(
        sema_io::block_on_ops() > ops_before,
        "sync http/get must enter the seam's block_on tier (ops stayed at {ops_before})"
    );

    // ── identity: exactly ONE pool served all of the above ─────────────────
    assert_eq!(
        sema_io::pools_built(),
        1,
        "every offload kind must share THE one sema-io pool"
    );
}
