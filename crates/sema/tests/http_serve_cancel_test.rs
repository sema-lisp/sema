//! SRV-1 ownership and cancellation gates for `http/serve`.
//!
//! This is IN-PROCESS (drives the `Interpreter`/`Runtime` host API directly,
//! not a subprocess), so it can distinguish server-root ownership from one
//! request's handler ownership and use runtime task counts as the leak oracle.

#![cfg(not(target_arch = "wasm32"))]

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use sema_core::runtime::CancelReason;
use sema_core::{NativeFn, Value};
use sema_eval::Interpreter;
use sema_vm::runtime::{RootOptions, RootPoll};

fn wait_until_rebindable(port: u16, deadline: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                return true;
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("rebind loopback listener {port}: {error}"),
        }
    }
    false
}

fn install_on_listen_signal(interp: &Interpreter, tx: mpsc::Sender<u16>) {
    interp.global_env.set_str(
        "test/on-listen",
        Value::native_fn(NativeFn::simple("test/on-listen", move |args| {
            let port = args
                .first()
                .and_then(Value::as_map_rc)
                .and_then(|info| info.get(&Value::keyword("port")).cloned())
                .and_then(|value| value.as_int())
                .and_then(|port| u16::try_from(port).ok())
                .expect("http/serve on-listen supplies a u16 port");
            let _ = tx.send(port);
            Ok(Value::nil())
        })),
    );
}

fn http_get_body(port: u16, path: &str, timeout: Duration) -> Result<String, String> {
    let addr = format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or("no loopback address")?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|error| error.to_string())?;
    Ok(raw
        .split_once("\r\n\r\n")
        .map_or_else(String::new, |(_, body)| body.to_string()))
}

#[cfg(unix)]
fn reset_connection(stream: TcpStream) -> io::Result<()> {
    use std::mem::size_of;
    use std::os::fd::AsRawFd;

    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    let result = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&linger as *const libc::linger).cast(),
            size_of::<libc::linger>() as libc::socklen_t,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    drop(stream);
    Ok(())
}

#[test]
fn cancelling_server_root_releases_listener_for_rebind() {
    let interp = Interpreter::new();
    let (port_tx, port_rx) = mpsc::channel();
    install_on_listen_signal(&interp, port_tx);

    let handle = interp
        .submit_str(
            r#"(http/serve
                  (fn (req) (http/text "ok"))
                  {:host "127.0.0.1" :port 0 :on-listen test/on-listen})"#,
            RootOptions::default(),
        )
        .expect("http/serve submits as a root");
    let root_id = handle.id();
    let cmd = interp.command_handle();
    let canceller = thread::spawn(move || {
        let port = port_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("on-listen reports the OS-assigned port");
        let cancelled = cmd.cancel_root(root_id);
        (port, cancelled)
    });

    let result = interp.drive_until_settled(&handle);
    let (port, cancelled) = canceller.join().expect("canceller thread completes");
    let rebound = wait_until_rebindable(port, Duration::from_secs(2));

    assert!(cancelled, "cancel_root accepts the live server root");
    assert!(result.is_err(), "cancelled server root settles as an error");
    assert_eq!(interp.runtime_live_task_count(), 0, "server root is reaped");
    assert!(
        rebound,
        "dropping the server root must abort axum::serve and release port {port}"
    );
}

#[cfg(unix)]
#[test]
fn dropping_one_http_request_cancels_only_its_handler() {
    let interp = Interpreter::new();
    let (port_tx, port_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    install_on_listen_signal(&interp, port_tx);
    interp.global_env.set_str(
        "test/handler-entered",
        Value::native_fn(NativeFn::simple("test/handler-entered", move |_| {
            let _ = entered_tx.send(());
            Ok(Value::nil())
        })),
    );

    let handle = interp
        .submit_str(
            r#"(http/serve
                  (fn (req)
                    (if (= (:path req) "/drop")
                        (begin
                          (test/handler-entered)
                          (async/sleep 60000)
                          (http/text "late"))
                        (http/text "healthy")))
                  {:host "127.0.0.1" :port 0 :on-listen test/on-listen})"#,
            RootOptions::default(),
        )
        .expect("http/serve submits as a root");
    let (reset_tx, reset_rx) = mpsc::channel();
    let client = thread::spawn(move || -> Result<(u16, String), String> {
        let port = port_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        stream
            .write_all(b"GET /drop HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .map_err(|error| error.to_string())?;
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        reset_connection(stream).map_err(|error| error.to_string())?;
        let _ = reset_tx.send(());
        let health = http_get_body(port, "/health", Duration::from_secs(3))?;
        Ok((port, health))
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let reset_observed = loop {
        if reset_rx.try_recv().is_ok() {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        interp.drive_turn().expect("server runtime drive succeeds");
        thread::sleep(Duration::from_millis(1));
    };
    let handler_reaped = loop {
        if interp.runtime_live_task_count() == 1 {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        interp.drive_turn().expect("server runtime drive succeeds");
        thread::sleep(Duration::from_millis(1));
    };

    while !client.is_finished() && Instant::now() < deadline {
        interp.drive_turn().expect("server runtime drive succeeds");
        thread::sleep(Duration::from_millis(1));
    }
    let server_remained_live = matches!(handle.poll_result(), RootPoll::Pending);
    let _ = handle.cancel(CancelReason::Explicit);
    let cancelled = interp.drive_until_settled(&handle);
    let client_result = client.join().expect("client thread completes");

    assert!(
        reset_observed,
        "client reset occurs after the handler entry signal"
    );
    assert!(
        handler_reaped,
        "request-future drop must reap only the disconnected request handler"
    );
    assert!(
        server_remained_live,
        "request cancellation keeps the server root live"
    );
    let (_port, health) = client_result.expect("health request succeeds after reset");
    assert_eq!(
        health, "healthy",
        "server handles a sibling request after reset"
    );
    assert!(
        cancelled.is_err(),
        "explicit server cleanup cancels the root"
    );
    assert_eq!(
        interp.runtime_live_task_count(),
        0,
        "cleanup reaps every task"
    );
}

/// Cancelling the `http/serve` root while a handler task is parked mid-`async/
/// sleep` must reap BOTH the accept-loop root task AND the spawned handler
/// task — `runtime_live_task_count()` returns to 0, proving no orphan.
#[test]
fn cancel_during_slow_handler_reaps_the_handler_task_no_leak() {
    let interp = Interpreter::new();
    let (port_tx, port_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    install_on_listen_signal(&interp, port_tx);
    interp.global_env.set_str(
        "test/handler-entered",
        Value::native_fn(NativeFn::simple("test/handler-entered", move |_| {
            let _ = entered_tx.send(());
            Ok(Value::nil())
        })),
    );
    let handle = interp
        .submit_str(
            r#"(http/serve
                  (fn (req)
                    (begin
                      (test/handler-entered)
                      (async/sleep 60000)
                      (http/text "late")))
                  {:host "127.0.0.1" :port 0 :on-listen test/on-listen})"#,
            RootOptions::default(),
        )
        .expect("http/serve submits as a root");
    let root_id = handle.id();
    let cmd = interp.command_handle();

    let canceller = thread::spawn(move || {
        let port = port_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("on-listen reports the OS-assigned port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect slow request");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .expect("write slow request");
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("handler publishes entry before root cancellation");
        let cancelled = cmd.cancel_root(root_id);
        drop(stream);
        cancelled
    });

    // Blocks on THIS thread until the root settles — driving the runtime
    // (accepting the connection, spawning the handler task, parking it on
    // the sleep) the whole time, and unblocking promptly once the canceller
    // thread's `cancel_root` rides the same completion inbox.
    let result = interp.drive_until_settled(&handle);

    let cancelled = canceller.join().expect("canceller thread completes");
    assert!(
        cancelled,
        "cancel_root must accept the live http/serve root"
    );
    assert!(
        result.is_err(),
        "a cancelled http/serve root must settle as an error, not a value"
    );
    assert_eq!(
        interp.runtime_live_task_count(),
        0,
        "cancelling the server root must reap the in-flight handler task too — no leak"
    );
}

/// SRV-1 piece c: cancelling `http/serve` while a WebSocket handler is parked
/// IDLE in `(:recv conn)` on its External watch-generation wait must reap the
/// connection's handler task cleanly too — no orphan, no hang. This is the
/// cancellation half of the piece-c contract; `idle_websocket_does_not_block_`
/// `plain_request` (`http_serve_concurrent_test.rs`) covers liveness and the
/// corresponding message wake. Using a real `tungstenite` client ensures the
/// connection upgrades and the handler genuinely parks in `ws/recv`; a bare
/// TCP connect/disconnect never reaches that code path.
#[test]
fn cancel_during_idle_websocket_recv_reaps_the_handler_task_no_leak() {
    let interp = Interpreter::new();
    let (port_tx, port_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    install_on_listen_signal(&interp, port_tx);
    interp.global_env.set_str(
        "test/ws-entered",
        Value::native_fn(NativeFn::simple("test/ws-entered", move |_| {
            let _ = entered_tx.send(());
            Ok(Value::nil())
        })),
    );
    let program = r#"(http/serve
             (fn (req)
               (if (= (:path req) "/ws")
                   (http/websocket
                     (fn (conn)
                       (test/ws-entered)
                       ((:recv conn))))
                   (http/text "unused")))
             {:host "127.0.0.1" :port 0 :on-listen test/on-listen})"#;
    let handle = interp
        .submit_str(program, RootOptions::default())
        .expect("http/serve submits as a root");
    let root_id = handle.id();
    let cmd = interp.command_handle();
    let (release_socket_tx, release_socket_rx) = mpsc::channel::<()>();
    let (socket_dropped_tx, socket_dropped_rx) = mpsc::channel::<()>();

    let canceller = thread::spawn(move || {
        let port = port_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("on-listen reports the OS-assigned port");
        let ws_url = format!("ws://127.0.0.1:{port}/ws");
        let ws = tungstenite::connect(&ws_url);
        assert!(ws.is_ok(), "WS upgrade failed: {:?}", ws.err());
        let (socket, _resp) = ws.unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("WebSocket handler publishes entry before root cancellation");
        let cancelled = cmd.cancel_root(root_id);
        // Keep the socket alive until the VM thread has sampled its live-task
        // count. Sender drop also releases this receive if the main test exits.
        let _ = release_socket_rx.recv();
        drop(socket);
        let _ = socket_dropped_tx.send(());
        cancelled
    });

    let result = interp.drive_until_settled(&handle);
    let live_while_socket_held = interp.runtime_live_task_count();
    let socket_was_held = matches!(socket_dropped_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    // Complete thread/socket cleanup before any assertion can panic.
    let _ = release_socket_tx.send(());
    drop(release_socket_tx);
    let cancelled = canceller.join().expect("canceller thread completes");

    assert!(
        socket_was_held,
        "the client socket must remain held while live-task count is sampled"
    );
    assert!(
        cancelled,
        "cancel_root must accept the live http/serve root"
    );
    assert!(
        result.is_err(),
        "a cancelled http/serve root must settle as an error, not a value"
    );
    assert_eq!(
        live_while_socket_held,
        0,
        "cancelling the server root must reap the idle WebSocket handler while the client socket remains open"
    );
}
