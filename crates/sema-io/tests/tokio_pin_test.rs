//! Tokio-assumption pin tests (ADR #69). These pin the EMPIRICAL threading
//! contract the consolidated-pool design rests on — probed on tokio 1.50.0, the
//! workspace-resolved version — so a future tokio upgrade that changes the
//! rules fails loudly here instead of deadlocking or panicking in production.
//!
//! - (i) `block_on` via the pool from its own `spawn_blocking` thread: OK —
//!   the literal production shape (VM thread → offload → provider
//!   `complete()` → `io_block_on(reqwest)`).
//! - (ii) `block_on` from an async WORKER thread: panics.
//! - (iii) `block_on` from a plain OS thread: OK.
//! - (iv) nested `spawn_blocking → block_on → spawn_blocking` fan-out
//!   deadlocks at blocking-cap == N units and completes at 2N — the probe-h
//!   regression the admission semaphore exists to prevent.
//! - (v) oversubscription (600 offload units through the admission semaphore,
//!   each driving `block_on`) completes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::Duration;

/// (i) `io_block_on` from one of THE pool's own `spawn_blocking` closures
/// completes: `block_on` drives the future on the calling (blocking-tier)
/// thread; the pool's workers supply only the reactor/timers.
#[test]
fn block_on_from_own_spawn_blocking_completes() {
    let (tx, rx) = mpsc::channel();
    sema_io::io_spawn_blocking(move || {
        let v = sema_io::io_block_on(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            42
        });
        let _ = tx.send(v);
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10))
            .expect("spawn_blocking → block_on unit must complete"),
        42
    );
}

/// (ii) `io_block_on` from an async worker thread panics. Asserts only that a
/// panic OCCURRED, not the wording — on tokio 1.50.0 the message begins
/// "Cannot start a runtime from within a runtime", but the exact text is not
/// part of the contract.
#[test]
fn block_on_from_async_worker_panics() {
    let (tx, rx) = mpsc::channel();
    let _hook = sema_io::io_spawn(async move {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sema_io::io_block_on(async {})
        }));
        let _ = tx.send(result.is_err());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(10))
            .expect("worker probe must report"),
        "block_on from an async worker thread must panic"
    );
}

/// (iii) `io_block_on` from a plain OS thread (the VM thread's shape) completes.
#[test]
fn block_on_from_plain_thread_completes() {
    let v = sema_io::io_block_on(async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        7
    });
    assert_eq!(v, 7);
}

/// Run `units` nested `spawn_blocking → block_on → spawn_blocking` fan-out
/// units on a SEPARATE runtime with `blocking_cap` blocking threads, returning
/// whether they all completed within `timeout`. A barrier guarantees all outer
/// units hold their blocking slots simultaneously before any inner
/// `spawn_blocking` is submitted, making the at-cap deadlock deterministic. On
/// deadlock the driver thread (and its runtime) is leaked — the watchdog
/// `recv_timeout` is what keeps the TEST from hanging.
fn nested_fanout_completes(blocking_cap: usize, units: usize, timeout: Duration) -> bool {
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_blocking_threads(blocking_cap)
            .build()
            .expect("probe runtime");
        let handle = rt.handle().clone();
        let barrier = Arc::new(Barrier::new(units));
        let joins: Vec<_> = (0..units)
            .map(|_| {
                let h = handle.clone();
                let b = barrier.clone();
                rt.spawn_blocking(move || {
                    // All outer units occupy their blocking slot before any
                    // inner spawn_blocking can be serviced.
                    b.wait();
                    h.block_on(async {
                        let inner = tokio::task::spawn_blocking(|| {
                            std::thread::sleep(Duration::from_millis(10));
                        });
                        let _ = inner.await;
                    });
                })
            })
            .collect();
        rt.block_on(async {
            for j in joins {
                let _ = j.await;
            }
        });
        let _ = done_tx.send(());
        // Deadlocked runtimes never reach here; completed ones drop cleanly.
    });
    done_rx.recv_timeout(timeout).is_ok()
}

/// (iv) Probe h: with blocking-cap == N, N nested fan-out units deadlock (each
/// holds a slot while its inner spawn_blocking waits for one); with cap 2N they
/// complete. This is the regression the admission semaphore makes structurally
/// unreachable on THE pool.
#[test]
fn nested_fanout_deadlocks_at_cap_and_completes_at_double() {
    const N: usize = 4;
    assert!(
        !nested_fanout_completes(N, N, Duration::from_secs(2)),
        "expected DEADLOCK at blocking-cap == units (tokio changed the rules?)"
    );
    assert!(
        nested_fanout_completes(2 * N, N, Duration::from_secs(30)),
        "expected completion at blocking-cap == 2x units"
    );
}

/// (v) Probe s: 600 concurrent offload units (beyond the 448 admission
/// permits), each driving `io_block_on(sleep)`, all complete — excess units
/// queue on the semaphore instead of deadlocking the blocking tier.
#[test]
fn oversubscription_through_admission_semaphore_completes() {
    const UNITS: usize = 600;
    let done = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel();
    for _ in 0..UNITS {
        let tx = tx.clone();
        let done = done.clone();
        sema_io::io_spawn_blocking(move || {
            sema_io::io_block_on(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
            });
            done.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(());
        });
    }
    drop(tx);
    for i in 0..UNITS {
        rx.recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| {
                panic!(
                    "oversubscription stalled after {i} completions (done = {})",
                    done.load(Ordering::SeqCst)
                )
            });
    }
    assert_eq!(done.load(Ordering::SeqCst), UNITS);
}
