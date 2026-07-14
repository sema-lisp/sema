mod common;

#[cfg(unix)]
use common::watchdog::run_command_with_timeout;
use common::watchdog::run_sema_with_timeout;
use std::time::Duration;
#[cfg(unix)]
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
        r#"(println (string/repeat "x" {}))"#,
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
    assert!(
        run.stderr.len() <= DIAGNOSTIC_CAPTURE_LIMIT,
        "stderr diagnostics exceeded the capture limit"
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
