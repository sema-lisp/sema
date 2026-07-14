mod common;

use common::watchdog::run_sema_with_timeout;
use std::time::Duration;

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
