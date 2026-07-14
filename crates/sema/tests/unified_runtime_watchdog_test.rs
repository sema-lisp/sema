mod common;

#[cfg(any(unix, windows))]
use common::watchdog::run_command_with_timeout;
use common::watchdog::run_sema_with_timeout;
#[cfg(windows)]
use std::process::{Command, Stdio};
use std::time::Duration;
#[cfg(any(unix, windows))]
use std::time::Instant;

const DIAGNOSTIC_CAPTURE_LIMIT: usize = 64 * 1024;

#[test]
fn ready_spinner_does_not_starve_due_timer() {
    let run = run_sema_with_timeout(
        r#"
        (define spinner
          (async
            (let loop ()
              (async/sleep 0)
              (loop))))
        (define timer (async (async/sleep 1) :timer-fired))
        (define winner (async/race (list spinner timer)))
        (define cancelled-before-explicit-stop (async/cancelled? spinner))
        (async/cancel spinner)
        (println (list winner cancelled-before-explicit-stop))
        "#,
        Duration::from_secs(10),
    );

    assert!(!run.timed_out, "scheduler hung; stderr:\n{}", run.stderr);
    assert!(
        run.status.success(),
        "scheduler failed; stdout:\n{}\nstderr:\n{}",
        run.stdout,
        run.stderr
    );
    assert!(
        run.stdout.contains("(:timer-fired #f)"),
        "expected timer win without implicit race cancellation; stdout:\n{}",
        run.stdout
    );
}

#[test]
fn noisy_child_is_drained_without_hanging_and_capture_is_bounded() {
    let source = format!(
        r#"
        (define noisy (string/repeat "x" {}))
        (println noisy)
        (println-error noisy)
        "#,
        DIAGNOSTIC_CAPTURE_LIMIT * 4
    );
    let run = run_sema_with_timeout(&source, Duration::from_secs(5));

    assert!(!run.timed_out, "noisy child was misclassified as hung");
    assert!(
        run.status.success(),
        "noisy child failed; stderr:\n{}",
        run.stderr
    );
    assert_eq!(
        run.stdout.len(),
        DIAGNOSTIC_CAPTURE_LIMIT,
        "stdout diagnostics must be retained only up to the capture limit"
    );
    assert_eq!(
        run.stderr.len(),
        DIAGNOSTIC_CAPTURE_LIMIT,
        "stderr diagnostics must be drained and retained up to the capture limit"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess helper for escaped-session watchdog regression"]
fn escaped_pipe_writer_helper() {
    use std::io::Write;

    let mut ready_pipe = [-1; 2];
    let pipe_result = unsafe { libc::pipe(ready_pipe.as_mut_ptr()) };
    assert_eq!(
        pipe_result,
        0,
        "create escaped pipe writer readiness pipe: {}",
        std::io::Error::last_os_error()
    );

    let escaped_pid = unsafe { libc::fork() };
    if escaped_pid < 0 {
        unsafe {
            libc::close(ready_pipe[0]);
            libc::close(ready_pipe[1]);
        }
        panic!(
            "fork escaped pipe writer: {}",
            std::io::Error::last_os_error()
        );
    }
    if escaped_pid == 0 {
        unsafe {
            if libc::close(ready_pipe[0]) == -1 {
                libc::_exit(10);
            }
            if libc::setsid() == -1 {
                libc::_exit(11);
            }
            let ready = [1_u8];
            if libc::write(ready_pipe[1], ready.as_ptr().cast(), ready.len()) != 1 {
                libc::_exit(12);
            }
            if libc::close(ready_pipe[1]) == -1 {
                libc::_exit(13);
            }
            libc::sleep(2);
            libc::_exit(0);
        }
    }

    let close_result = unsafe { libc::close(ready_pipe[1]) };
    assert_eq!(
        close_result,
        0,
        "close parent readiness writer: {}",
        std::io::Error::last_os_error()
    );
    let mut ready = [0_u8];
    let read_result = loop {
        let result = unsafe { libc::read(ready_pipe[0], ready.as_mut_ptr().cast(), ready.len()) };
        if result != -1 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break result;
        }
    };
    assert_eq!(
        read_result,
        1,
        "escaped pipe writer did not report readiness: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(ready, [1], "escaped pipe writer readiness byte");
    let close_result = unsafe { libc::close(ready_pipe[0]) };
    assert_eq!(
        close_result,
        0,
        "close parent readiness reader: {}",
        std::io::Error::last_os_error()
    );

    println!("ESCAPED_PID={escaped_pid}");
    eprintln!("ESCAPED_STDERR_PID={escaped_pid}");
    std::io::stdout().flush().expect("flush escaped pid stdout");
    std::io::stderr().flush().expect("flush escaped pid stderr");
    std::process::exit(0);
}

#[cfg(unix)]
fn marked_pid(output: &str, marker: &str) -> libc::pid_t {
    output
        .split_whitespace()
        .find_map(|part| part.strip_prefix(marker))
        .expect("helper output must contain escaped pid marker")
        .parse()
        .expect("escaped pid marker must be numeric")
}

#[cfg(unix)]
fn terminate_escaped_helper(pid: libc::pid_t) {
    let result = unsafe { libc::kill(pid, libc::SIGKILL) };
    if result == -1 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "kill escaped helper {pid}"
        );
    }
}

#[cfg(unix)]
#[test]
fn escaped_session_pipe_writers_do_not_block_drain_join() {
    let executable = std::env::current_exe().expect("resolve watchdog test executable");
    let executable = executable.to_str().expect("test executable path is UTF-8");
    let started = Instant::now();
    let run = run_command_with_timeout(
        executable,
        &[
            "--ignored",
            "--exact",
            "escaped_pipe_writer_helper",
            "--nocapture",
        ],
        Duration::from_secs(1),
    );
    let elapsed = started.elapsed();
    let escaped_pid = marked_pid(&run.stdout, "ESCAPED_PID=");
    let escaped_stderr_pid = marked_pid(&run.stderr, "ESCAPED_STDERR_PID=");
    terminate_escaped_helper(escaped_pid);

    assert_eq!(
        escaped_stderr_pid, escaped_pid,
        "both pipes share one helper"
    );
    assert!(!run.timed_out, "the direct helper exited normally");
    assert!(run.status.success(), "helper failed: {}", run.stderr);
    assert!(
        elapsed < Duration::from_secs(1),
        "escaped pipe writers blocked drain joins for {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn inherited_pipe_writer_does_not_extend_parent_watchdog() {
    let started = Instant::now();
    let run = run_command_with_timeout(
        "sh",
        &["-c", "sleep 10 & descendant=$!; echo $descendant"],
        Duration::from_secs(1),
    );

    assert!(!run.timed_out, "the watched shell exited normally");
    assert!(run.status.success(), "watched shell failed: {}", run.stderr);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an inherited pipe writer kept the watchdog blocked for {:?}",
        started.elapsed()
    );

    let descendant = run
        .stdout
        .trim()
        .parse::<libc::pid_t>()
        .expect("shell must report its background descendant pid");
    let reap_deadline = Instant::now() + Duration::from_secs(1);
    while unsafe { libc::kill(descendant, 0) } == 0 && Instant::now() < reap_deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let descendant_lookup = unsafe { libc::kill(descendant, 0) };
    let descendant_lookup_error = std::io::Error::last_os_error();
    assert_eq!(
        descendant_lookup, -1,
        "watchdog process-group cleanup left descendant {descendant} alive"
    );
    assert_eq!(
        descendant_lookup_error.raw_os_error(),
        Some(libc::ESRCH),
        "descendant lookup should fail because process-group cleanup reaped it"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "subprocess helper for Windows inherited-pipe watchdog regression"]
fn windows_inherited_pipe_writer_helper() {
    use std::io::Write;

    let child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 2",
        ])
        .spawn()
        .expect("spawn Windows inherited-pipe writer");
    println!("INHERITED_PID={}", child.id());
    eprintln!("INHERITED_STDERR_PID={}", child.id());
    std::io::stdout()
        .flush()
        .expect("flush inherited pid stdout");
    std::io::stderr()
        .flush()
        .expect("flush inherited pid stderr");
}

#[cfg(windows)]
fn marked_windows_pid(output: &str, marker: &str) -> u32 {
    output
        .split_whitespace()
        .find_map(|part| part.strip_prefix(marker))
        .expect("helper output must contain inherited pid marker")
        .parse()
        .expect("inherited pid marker must be numeric")
}

#[cfg(windows)]
#[test]
fn windows_inherited_pipe_writer_does_not_block_drain_join() {
    let executable = std::env::current_exe().expect("resolve watchdog test executable");
    let executable = executable.to_str().expect("test executable path is UTF-8");
    let started = Instant::now();
    let run = run_command_with_timeout(
        executable,
        &[
            "--ignored",
            "--exact",
            "windows_inherited_pipe_writer_helper",
            "--nocapture",
        ],
        Duration::from_secs(1),
    );
    let elapsed = started.elapsed();
    let inherited_pid = marked_windows_pid(&run.stdout, "INHERITED_PID=");
    let inherited_stderr_pid = marked_windows_pid(&run.stderr, "INHERITED_STDERR_PID=");

    let _ = Command::new("taskkill.exe")
        .args(["/PID", &inherited_pid.to_string(), "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    assert_eq!(
        inherited_stderr_pid, inherited_pid,
        "both pipes share one inherited writer"
    );
    assert!(!run.timed_out, "the direct helper exited normally");
    assert!(run.status.success(), "helper failed: {}", run.stderr);
    assert!(
        elapsed < Duration::from_secs(1),
        "a Windows inherited pipe writer blocked drain joins for {elapsed:?}"
    );
}
