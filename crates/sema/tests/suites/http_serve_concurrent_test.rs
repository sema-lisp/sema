//! Structural SRV-1 acceptance gates for concurrent `http/serve` dispatch.
//!
//! Server readiness and handler entry travel over explicit stdout markers.
//! Wall-clock limits are watchdogs only: no assertion compares elapsed time.
//! Every subprocess is owned by `ServeProcess`, whose Drop terminates and
//! waits for the child (and its Unix process group) before joining the marker
//! reader thread.

#![cfg(not(target_arch = "wasm32"))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const BOUND_SIGNAL: &str = "SRV1_BOUND:";

#[derive(Debug, Eq, PartialEq)]
enum HarnessWaitError {
    Timeout,
    ChildExited,
}

struct ServeProcess {
    child: Option<Child>,
    lines: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    port: Option<u16>,
    reaped: Arc<AtomicBool>,
}

impl ServeProcess {
    fn spawn_raw(program: &str, reaped: Arc<AtomicBool>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sema"));
        command
            .args(["--no-llm", "-e", program])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().expect("spawn sema server subprocess");
        let stdout = child.stdout.take().expect("take sema server stdout");
        let (line_tx, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child: Some(child),
            lines,
            reader: Some(reader),
            port: None,
            reaped,
        }
    }

    fn spawn_with_on_listen(handler: &str, on_listen: &str) -> Self {
        let program = format!(
            r#"(http/serve
                  {handler}
                  {{:host "127.0.0.1"
                    :port 0
                    :on-listen {on_listen}}})"#
        );
        let mut process = Self::spawn_raw(&program, Arc::new(AtomicBool::new(false)));
        let port = process
            .wait_for_signal(BOUND_SIGNAL, Duration::from_secs(5))
            .expect("server reports its OS-assigned port")
            .parse::<u16>()
            .expect("BOUND signal contains a u16 port");
        process.port = Some(port);
        process
    }

    fn spawn(handler: &str) -> Self {
        let on_listen = format!(
            r#"(fn (info)
                  (println (string-append "{BOUND_SIGNAL}"
                                          (number->string (:port info)))))"#
        );
        Self::spawn_with_on_listen(handler, &on_listen)
    }

    fn id(&self) -> u32 {
        self.child.as_ref().expect("live child").id()
    }

    fn port(&self) -> u16 {
        self.port.expect("serve process has a bound port")
    }

    fn wait_for_signal(
        &mut self,
        prefix: &str,
        timeout: Duration,
    ) -> Result<String, HarnessWaitError> {
        let deadline = Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(HarnessWaitError::Timeout);
            };
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    if let Some(value) = line.strip_prefix(prefix) {
                        return Ok(value.to_string());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(HarnessWaitError::Timeout);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(HarnessWaitError::ChildExited);
                }
            }
        }
    }

    fn terminate(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let pid = child.id();
        terminate_child(&mut child, pid);
        let _ = child.wait();
        #[cfg(unix)]
        terminate_process_group(pid);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.reaped.store(true, Ordering::Release);
    }
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    let result = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    if result == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
        // Drop cleanup is best-effort; the direct child kill below is the
        // portable fallback and wait still prevents a zombie.
    }
}

#[cfg(unix)]
fn terminate_child(child: &mut Child, pid: u32) {
    terminate_process_group(pid);
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
}

#[cfg(not(unix))]
fn terminate_child(child: &mut Child, _pid: u32) {
    let _ = child.kill();
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn http_get_body(port: u16, path: &str, timeout: Duration) -> Result<String, String> {
    http_get_status_and_body(port, path, timeout).map(|(_, body)| body)
}

fn http_get_status_and_body(
    port: u16,
    path: &str,
    timeout: Duration,
) -> Result<(u16, String), String> {
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
    let (status_line, rest) = raw.split_once("\r\n").ok_or("no HTTP status line")?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or("no HTTP status code")?
        .parse::<u16>()
        .map_err(|error| error.to_string())?;
    let body = rest
        .split_once("\r\n\r\n")
        .map_or_else(String::new, |(_, body)| body.to_string());
    Ok((status, body))
}

#[test]
fn slow_handler_does_not_block_fast_handler() {
    let mut server = ServeProcess::spawn(
        r#"(fn (req)
              (if (= (:path req) "/slow")
                  (begin
                    (println "SRV1_SLOW_ENTERED")
                    (async/sleep 60000)
                    (http/text "slow"))
                  (http/text "fast")))"#,
    );
    let port = server.port();
    let slow = thread::spawn(move || http_get_body(port, "/slow", Duration::from_secs(10)));
    server
        .wait_for_signal("SRV1_SLOW_ENTERED", Duration::from_secs(5))
        .expect("slow handler publishes entry before the fast request");

    let fast = http_get_body(port, "/fast", Duration::from_secs(3));
    server.terminate();
    let _ = slow.join();

    assert_eq!(fast.as_deref(), Ok("fast"));
}

#[test]
fn idle_websocket_does_not_block_plain_request() {
    let mut server = ServeProcess::spawn(
        r#"(fn (req)
              (if (= (:path req) "/ws")
                  (http/websocket
                    (fn (conn)
                      (println "SRV1_WS_ENTERED")
                      (let loop ()
                        (let ((msg ((:recv conn))))
                          (if (null? msg)
                              nil
                              (begin ((:send conn) msg) (loop)))))))
                  (http/text "pong")))"#,
    );
    let port = server.port();
    let ws_url = format!("ws://127.0.0.1:{port}/ws");
    let (mut socket, _response) = tungstenite::connect(&ws_url).expect("WebSocket upgrade");
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("bound WebSocket read"),
        _ => panic!("loopback ws:// connection uses plain TCP"),
    }
    server
        .wait_for_signal("SRV1_WS_ENTERED", Duration::from_secs(5))
        .expect("WebSocket handler publishes entry before the sibling request");

    let ping = http_get_body(port, "/ping", Duration::from_secs(3));
    let echo = socket
        .send(tungstenite::Message::Text("generation-wake".into()))
        .and_then(|()| socket.read());
    server.terminate();

    assert_eq!(ping.as_deref(), Ok("pong"));
    assert_eq!(
        echo.expect("echo frame").into_text().expect("echo is text"),
        "generation-wake"
    );
}

#[test]
fn suspended_sse_handler_does_not_block_plain_request() {
    let mut server = ServeProcess::spawn(
        r#"(fn (req)
              (if (= (:path req) "/events")
                  (http/stream
                    (fn (send)
                      (println "SRV1_SSE_ENTERED")
                      (async/sleep 60000)
                      (send "late")))
                  (http/text "pong")))"#,
    );
    let port = server.port();
    let events = thread::spawn(move || http_get_body(port, "/events", Duration::from_secs(10)));
    server
        .wait_for_signal("SRV1_SSE_ENTERED", Duration::from_secs(5))
        .expect("SSE handler publishes entry before the sibling request");

    let ping = http_get_body(port, "/ping", Duration::from_secs(3));
    server.terminate();
    let _ = events.join();

    assert_eq!(ping.as_deref(), Ok("pong"));
}

#[test]
fn suspended_sse_handler_resumes_and_closes_stream() {
    let mut server = ServeProcess::spawn(
        r#"(fn (_req)
              (http/stream
                (fn (send)
                  (async/sleep 10)
                  (send "resumed"))))"#,
    );

    let body = http_get_body(server.port(), "/events", Duration::from_secs(3));
    server.terminate();

    assert!(
        body.as_deref()
            .is_ok_and(|body| body.contains("data: resumed")),
        "SSE handler must resume, publish its event, and close: {body:?}"
    );
}

#[test]
fn handler_parking_on_async_returns_response() {
    let mut server = ServeProcess::spawn(
        r#"(fn (req)
              (http/text
                (async/await
                  (async/spawn (fn () (begin (async/sleep 10) "awaited"))))))"#,
    );
    let body = http_get_body(server.port(), "/", Duration::from_secs(3));
    server.terminate();
    assert_eq!(body.as_deref(), Ok("awaited"));
}

#[test]
fn on_listen_callback_runs_through_the_cooperative_runtime() {
    let on_listen = format!(
        r#"(fn (info)
              (async/await (async/spawn (fn () "ready")))
              (println (string-append "{BOUND_SIGNAL}"
                                      (number->string (:port info)))))"#
    );
    let mut server =
        ServeProcess::spawn_with_on_listen(r#"(fn (_req) (http/text "ok"))"#, &on_listen);

    let body = http_get_body(server.port(), "/", Duration::from_secs(3));
    server.terminate();

    assert_eq!(body.as_deref(), Ok("ok"));
}

#[test]
fn regression_top_level_serve_still_answers() {
    let mut server = ServeProcess::spawn(r#"(fn (req) (http/text (:path req)))"#);
    let body = http_get_body(server.port(), "/echo-me", Duration::from_secs(3));
    server.terminate();
    assert_eq!(body.as_deref(), Ok("/echo-me"));
}

#[test]
fn uncaught_handler_error_produces_the_bounded_500_fallback() {
    let mut server = ServeProcess::spawn(r#"(fn (req) (error "boom"))"#);
    let result = http_get_status_and_body(server.port(), "/anything", Duration::from_secs(3));
    server.terminate();

    let (status, body) = result.expect("request completes with bounded fallback");
    assert_eq!(status, 500);
    assert_eq!(body, "Handler did not respond");
}

#[test]
fn harness_timeout_reaps_child_via_raii() {
    let reaped = Arc::new(AtomicBool::new(false));
    let pid;
    let timeout;
    {
        let mut process = ServeProcess::spawn_raw(
            r#"(begin (println "SRV1_CHILD_STARTED") (async/sleep 60000))"#,
            Arc::clone(&reaped),
        );
        pid = process.id();
        process
            .wait_for_signal("SRV1_CHILD_STARTED", Duration::from_secs(5))
            .expect("child reaches deliberate timeout fixture");
        timeout = process.wait_for_signal("SRV1_NEVER", Duration::from_millis(25));
    }

    assert_eq!(timeout, Err(HarnessWaitError::Timeout));
    assert!(
        reaped.load(Ordering::Acquire),
        "Drop killed and waited child"
    );
    #[cfg(unix)]
    assert!(!process_exists(pid), "wait reaped child pid {pid}");
}
