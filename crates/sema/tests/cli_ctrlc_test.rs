//! Out-of-process coverage for the native CLI's Ctrl-C → `RuntimeCommandHandle::
//! cancel_all` wiring (`install_ctrlc_handler` in `crates/sema/src/main.rs`).
//!
//! Unix-only: sends a real `SIGINT` to a spawned `sema` child, mirroring the
//! watchdog harness's out-of-process pattern
//! (`crates/sema/tests/unified_runtime_watchdog_test.rs`) but driving the
//! signal at a specific point mid-run rather than only killing on timeout.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

/// Grace period between spawning the child and sending `SIGINT`, giving
/// `install_ctrlc_handler` time to run before the signal arrives. A cold
/// first exec of the (large, asset-embedding) `sema` binary can take longer
/// than interpreter construction itself to page in from disk — `warm_up`
/// below pays that cost once, up front, so this grace period only has to
/// cover actual interpreter startup, not first-exec I/O.
const STARTUP_GRACE: Duration = Duration::from_millis(200);

/// Run the binary once, discarding output, so its pages are hot in the OS
/// cache before the timing-sensitive tests spawn it. Without this, the FIRST
/// test to exec a freshly built (100+ MB) `sema` binary can take longer than
/// `STARTUP_GRACE` just to page in, racing `SIGINT` against
/// `install_ctrlc_handler` and flaking as "killed by default SIGINT
/// disposition" instead of exercising the handler at all.
fn warm_up() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = Command::new(env!("CARGO_BIN_EXE_sema"))
            .args(["--no-llm", "-e", "(+ 1 2)"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

/// Send `SIGINT` to `pid` (a direct kill, not a process-group signal — the
/// child installs its own handler via `ctrlc`, which is what we're testing).
fn send_sigint(pid: u32) {
    let result = unsafe { libc::kill(pid as i32, libc::SIGINT) };
    assert_eq!(
        result,
        0,
        "kill -INT {pid}: {}",
        std::io::Error::last_os_error()
    );
}

/// Wait for `child` to exit, polling rather than blocking indefinitely so a
/// regression (Ctrl-C not wired up) fails the test instead of hanging CI.
fn wait_with_deadline(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            return Some(status);
        }
        if started.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A long `async/sleep` interrupted by a single `SIGINT` must settle the
/// cancelled root and exit well within the sleep's duration — proving Ctrl-C
/// routes through `cancel_all` rather than (not) being handled at all (the
/// pre-fix native CLI had no SIGINT wiring whatsoever, so this exact program
/// used to run for the full 60s; nothing on the native path installed the
/// interrupt callback that would have let `check_interrupt` polling help
/// either). Also regression-pins the companion fix in
/// `Interpreter::drive_handle_to_settlement` (crates/sema-eval/src/eval.rs):
/// a root parked purely on a timer used to block the drive loop in a raw
/// `thread::sleep`, which a cross-thread cancel command could not wake —
/// only the timer firing (or here, the full 60s) would. It now blocks on the
/// same inbox a parked external wait uses, bounded by the deadline, so a
/// `cancel_all` sent mid-sleep wakes it immediately.
#[test]
fn sigint_cancels_long_running_sleep_promptly() {
    warm_up();

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", "(async/sleep 60000)"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema");

    std::thread::sleep(STARTUP_GRACE);
    send_sigint(child.id());

    let status = wait_with_deadline(&mut child, Duration::from_millis(500)).unwrap_or_else(|| {
        let _ = child.kill();
        panic!("sema did not exit within 500ms of SIGINT (Ctrl-C not cancelling the runtime)");
    });

    assert!(
        !status.success(),
        "a Ctrl-C-cancelled program must not report success"
    );

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("child stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(
        stderr.contains("cancelled"),
        "expected the cancelled-settlement error on stderr, got: {stderr}"
    );
}

/// A program parked on an external subprocess (`shell`) must have that child
/// torn down when the root is cancelled — not left as an orphan. Spawns a
/// long-lived marker process; after SIGINT, asserts (a) `sema` exits promptly
/// and (b) the shelled-out process is no longer alive.
#[test]
fn sigint_tears_down_parked_subprocess() {
    warm_up();

    // A distinctive sleep duration makes it easy to find the marker process by
    // its argv without colliding with anything else running on the box.
    let marker = "424301";
    let program = format!(r#"(shell "sleep {marker}")"#);

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", &program])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema");

    // Give the shell child time to actually exec.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        pgrep_running(marker),
        "the shelled subprocess ('sleep {marker}') never started; test setup is broken"
    );

    send_sigint(child.id());

    let status = wait_with_deadline(&mut child, Duration::from_millis(500)).unwrap_or_else(|| {
        let _ = child.kill();
        panic!("sema did not exit within 500ms of SIGINT (Ctrl-C not cancelling the runtime)");
    });
    assert!(!status.success());

    // The teardown of the External wait's resource (killpg on the shell's
    // process group) is synchronous with root cancellation in the drive loop,
    // so by the time `sema` itself has exited the marker process must be gone
    // too. A short grace poll absorbs OS scheduling jitter without masking a
    // real leak (it would still be running seconds later).
    let started = Instant::now();
    while pgrep_running(marker) && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !pgrep_running(marker),
        "the shelled subprocess ('sleep {marker}') was not torn down by Ctrl-C cancellation"
    );
}

fn pgrep_running(marker: &str) -> bool {
    Command::new("pgrep")
        .arg("-f")
        .arg(format!("sleep {marker}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// A Sema program that installs its own `sys/on-signal :int` handler takes
/// ownership of `SIGINT` at the OS level (`libc::signal`, `crates/sema-stdlib
/// /src/system.rs`), which overwrites `install_ctrlc_handler`'s registration
/// outright (there is only one OS handler per signal). The host's automatic
/// Ctrl-C → `cancel_all` no longer applies once a program opts into raw
/// signal handling — same principle as any other host default a program can
/// override. A no-op callback here proves the takeover is total: the process
/// must survive well past where `sigint_cancels_long_running_sleep_promptly`
/// proves the host's own handler would have cancelled it.
///
/// The double-interrupt hard-exit escape hatch itself (see
/// `is_double_interrupt` in `crates/sema/src/main.rs`) is unit-tested
/// directly against its window arithmetic rather than through real signals
/// here: every scenario this suite can drive end-to-end — a bare timer sleep,
/// a parked subprocess — cancels promptly (well under the hard-exit's 2s
/// window), and the moment a Sema program installs its own handler (as
/// below), the SECOND signal never reaches `install_ctrlc_handler` either, so
/// there is no real "runtime ignored cancel_all and needed a hard kill"
/// scenario this harness can manufacture without depending on host CPU
/// scheduling.
#[test]
fn sema_level_signal_handler_overrides_host_default() {
    warm_up();

    let program = r#"
        (sys/on-signal :int (fn () nil))
        (async/sleep 60000)
    "#;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", program])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema");

    std::thread::sleep(STARTUP_GRACE);
    send_sigint(child.id());

    // Well past the ~5-30ms the host's own handler needs to cancel the same
    // `async/sleep` (see `sigint_cancels_long_running_sleep_promptly`): if
    // the process is still alive here, the Sema-level handler — not the
    // host's `cancel_all` — is the one that owns SIGINT.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().expect("poll child").is_none(),
        "process exited after a single SIGINT despite installing its own sys/on-signal handler"
    );

    child.kill().expect("kill still-alive child");
    let _ = child.wait();
}
