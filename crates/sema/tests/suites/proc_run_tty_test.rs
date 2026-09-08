//! Out-of-process coverage for `proc/run` (`crates/sema-stdlib/src/proc.rs`).
//!
//! `proc/run` hands the child the PARENT's terminal, so it can only be tested
//! from a process that actually has one: these tests run the `sema` binary
//! under a real pty (`portable_pty`, already used by `pty/spawn`) and read what
//! reaches the terminal, mirroring the out-of-process pattern in
//! `cli_ctrlc_test.rs`.
//!
//! Unix-only: the cooked-mode handoff is a `termios` operation, and the probe
//! programs are `sh -c 'stty …'`.

#![cfg(unix)]

use std::io::Read;
use std::time::{Duration, Instant};

/// Sema program driving one `proc/run` from inside `io/with-raw-mode`, printing
/// four facts the assertions below pin:
///   `child-tty:yes`   the child sees a terminal on fd 0 (it inherited ours)
///   `canon:0`         `stty` in the child reports icanon ON — a cooked
///                     terminal — even though the parent is in raw mode
///                     (`grep -c -- -icanon` counts the DISABLED form)
///   `exit: 3`         the child's exit code comes back
///   `raw-after: 1`    the parent is in raw mode again once the child exits
///
/// Every child exits on its own in milliseconds; none reads stdin.
const PROGRAM: &str = r#"
(define probe "stty -a </dev/tty | tr ' ' '\n' | grep -c -- '-icanon'")
(define (parent-raw?) (string/trim (get (shell probe) :stdout)))
(io/with-raw-mode
  (begin
    (proc/run ["sh" "-c" "[ -t 0 ] && printf 'child-tty:yes\r\n'"])
    (proc/run ["sh" "-c" (string-append "printf 'canon:'; " probe)])
    (println "exit:" (proc/run ["sh" "-c" "exit 3"]))
    (println "raw-after:" (parent-raw?))))
"#;

/// Read from `reader` until `needle` appears or `timeout` elapses. The pty
/// master stays open (the slave side is held by the child), so a plain
/// read-to-end would block past the program's exit — poll for the marker
/// instead, and let a regression fail the assertion rather than hang CI.
fn read_until(mut reader: Box<dyn Read + Send>, needle: &str, timeout: Duration) -> String {
    let started = Instant::now();
    let mut out = String::new();
    let mut buf = [0u8; 1024];
    while started.elapsed() < timeout {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
                if out.contains(needle) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    out
}

#[test]
fn proc_run_hands_the_child_the_parent_terminal() {
    let pty = match portable_pty::native_pty_system().openpty(portable_pty::PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pty) => pty,
        Err(_) => return, // no pty available (sandboxed CI) — nothing to test
    };

    let mut cmd = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_sema"));
    cmd.args(["--no-llm", "-e", PROGRAM]);
    let mut child = pty
        .slave
        .spawn_command(cmd)
        .expect("spawn sema under a pty");
    let reader = pty.master.try_clone_reader().expect("pty reader");
    drop(pty.slave);

    let out = read_until(reader, "raw-after:", Duration::from_secs(20));
    let _ = child.wait();

    assert!(
        out.contains("child-tty:yes"),
        "the child must inherit the parent's terminal on fd 0; got: {out:?}"
    );
    assert!(
        out.contains("canon:0"),
        "the child must run in cooked mode even though the parent is raw; got: {out:?}"
    );
    assert!(
        out.contains("exit: 3"),
        "proc/run must return the child's exit code; got: {out:?}"
    );
    assert!(
        out.contains("raw-after: 1"),
        "the parent's raw mode must be restored once the child exits; got: {out:?}"
    );
}

/// Without a terminal on fd 0 the call is refused rather than inheriting the
/// host's file descriptors — under `sema mcp` (stdio JSON-RPC) or a notebook
/// cell that would corrupt the protocol stream and block the server past any
/// eval deadline.
#[test]
fn proc_run_without_a_terminal_errors() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", r#"(proc/run ["echo" "hi"])"#])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run sema");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("stdin is not a terminal"),
        "expected the no-tty refusal; got: {text:?}"
    );
}
