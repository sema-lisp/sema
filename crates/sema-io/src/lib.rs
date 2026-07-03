//! THE process-wide tokio I/O pool, installed behind `sema_core`'s executor
//! seam (`io_backend.rs`) — ADR #69's "one pool behind one seam".
//!
//! Leaf crate: depends on `sema-core` and `tokio` only. Every native crate that
//! offloads I/O (sema-stdlib http/shell, sema-llm wire calls, `http/serve`)
//! reaches the pool exclusively through the [`io_spawn`] / [`io_spawn_blocking`]
//! / [`io_block_on`] wrappers below, which install the backend on first use —
//! the sanctioned entry points; the conformance test
//! (`runtime_conformance_test.rs` in the `sema-lang` crate) forbids both ad-hoc
//! runtime creation and raw `sema_core::io_*` calls anywhere else.
//!
//! # Pool shape
//!
//! One `new_multi_thread().enable_all()` runtime (`enable_all` is required:
//! `tokio::process` driver for shell offloads, timers for sleep-once and retry
//! backoff), `max_blocking_threads(512)`, threads named `sema-io-{n}` so tests
//! and profilers can attribute work to the pool.
//!
//! # Admission control (the probe-h deadlock, prevented by mechanism)
//!
//! Every blocking-tier offload unit ([`io_spawn_blocking`] and
//! [`io_offload_blocking`]) acquires a permit from [`OFFLOAD_SEM`] (448
//! permits) before entering the 512-slot blocking tier. The 64-slot headroom
//! exists because an offloaded unit may `io_block_on` a reqwest future whose
//! GaiResolver DNS lookup transiently needs ONE extra blocking slot: with
//! permits == slots, a full-cap burst of such units deadlocks (each holds a
//! slot while waiting for a slot); with depth-1 headroom reserved, that
//! deadlock is structurally unreachable. Excess offloads beyond 448 queue on
//! the semaphore — behavior at realistic fan-out is identical. A permit is
//! held for the closure's ENTIRE duration, including any retry-backoff
//! `thread::sleep`s inside it — long backoffs occupy a permit by design (they
//! occupy a blocking thread too).
//!
//! Futures spawned via [`io_spawn`] take NO permit: they run on the async
//! workers and pin no blocking slot while suspended, so the semaphore guards
//! only the tier that can exhaust blocking threads. An `io_spawn` future that
//! needs sync work inside (e.g. a sync-only LLM provider under the async
//! completion offload) goes through [`io_offload_blocking`], which is where
//! its permit is taken.
//!
//! # Threading contract
//!
//! Pinned by `tests/tokio_pin_test.rs` (re-established on every CI run so a
//! tokio upgrade that changes the rules fails loudly): `io_block_on` is legal
//! from plain OS threads (the VM thread) and from this pool's own
//! `spawn_blocking` closures — `block_on` drives the future on the CALLING
//! thread, workers supply only the reactor/timers — and panics from async
//! worker threads.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use sema_core::{AbortHook, BoxIoFuture, IoBackend};
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;

/// Cap on the pool's blocking-thread tier.
const MAX_BLOCKING_THREADS: usize = 512;

/// Admission permits for blocking-tier offload units (`io_spawn_blocking` /
/// `io_offload_blocking`). 64 below
/// [`MAX_BLOCKING_THREADS`]: depth-1 headroom for the one extra blocking slot a
/// `block_on`'d future may transiently need (GaiResolver DNS) — see the module
/// docs.
const OFFLOAD_PERMITS: usize = 448;

static POOL: OnceLock<Runtime> = OnceLock::new();

/// How many times the pool builder ran. `OnceLock` already guarantees at most
/// one; the counter is the identity oracle's observable
/// (`io_pool_identity_test.rs` asserts `pools_built() == 1` after driving every
/// offload kind).
static POOLS_BUILT: AtomicU64 = AtomicU64::new(0);

/// Seam-entry counter for the `block_on` tier. `block_on` drives the future on
/// the calling thread — pool thread names can never observe it — so the
/// identity oracle counts entries here instead.
static BLOCK_ON_OPS: AtomicU64 = AtomicU64::new(0);

static OFFLOAD_SEM: Semaphore = Semaphore::const_new(OFFLOAD_PERMITS);

fn pool() -> &'static Runtime {
    POOL.get_or_init(|| {
        POOLS_BUILT.fetch_add(1, Ordering::SeqCst);
        static THREAD_N: AtomicU64 = AtomicU64::new(0);
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_blocking_threads(MAX_BLOCKING_THREADS)
            .thread_name_fn(|| {
                let n = THREAD_N.fetch_add(1, Ordering::SeqCst);
                format!("sema-io-{n}")
            })
            .build()
            .expect("sema-io: failed to build the process-wide I/O pool")
    })
}

/// The blessed [`IoBackend`] over THE pool.
struct SemaIoBackend;

impl IoBackend for SemaIoBackend {
    fn spawn(&self, fut: BoxIoFuture) -> AbortHook {
        let abort = pool().spawn(fut).abort_handle();
        Box::new(move || abort.abort())
    }

    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send>) {
        // Admission control: the permit is acquired asynchronously (so callers
        // never block on a full tier) and held across the closure's entire run.
        pool().spawn(async move {
            let _permit = OFFLOAD_SEM
                .acquire()
                .await
                .expect("OFFLOAD_SEM is never closed");
            let _ = tokio::task::spawn_blocking(work).await;
        });
    }

    fn block_on_boxed(&self, fut: Pin<Box<dyn Future<Output = ()> + '_>>) {
        pool().block_on(fut);
    }
}

/// Install THE pool as the process-wide I/O backend. Idempotent, first-wins.
/// Called from `register_stdlib` (native), `register_llm_builtins`, and
/// `reset_runtime_state`, so library tests without a full interpreter still get
/// the one pool; the `io_*` wrappers below also call it, so any entry works.
pub fn install() {
    let _ = sema_core::set_io_backend(Box::new(SemaIoBackend));
}

/// Spawn a future on THE pool; returns a one-shot abort hook (dropping it does
/// NOT abort). The sanctioned entry for the `spawn` tier (async http/shell
/// offloads, `http/serve`).
pub fn io_spawn<F>(fut: F) -> AbortHook
where
    F: Future<Output = ()> + Send + 'static,
{
    install();
    sema_core::io_spawn(Box::pin(fut))
}

/// Offload a synchronous closure to THE pool's blocking tier, admission-
/// controlled (see the module docs). The sanctioned entry for the
/// `spawn_blocking` tier (LLM wire units, embed offloads).
pub fn io_spawn_blocking<F>(work: F)
where
    F: FnOnce() + Send + 'static,
{
    install();
    sema_core::io_spawn_blocking(Box::new(work));
}

/// Offload `work` to THE pool's blocking tier from ASYNC context and await its
/// result — the awaitable sibling of [`io_spawn_blocking`], admission-
/// controlled by the same [`OFFLOAD_SEM`] (see the module docs). The permit is
/// moved INTO the blocking closure so it is held for the closure's entire run
/// even if the awaiting future is aborted first. Cancellation here is
/// best-effort by construction: aborting the awaiting future discards the
/// result, but the closure cannot be interrupted and runs to completion on the
/// worker.
pub async fn io_offload_blocking<T, F>(work: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    install();
    let permit = OFFLOAD_SEM
        .acquire()
        .await
        .expect("OFFLOAD_SEM is never closed");
    match pool()
        .spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
    {
        Ok(v) => v,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(e) => panic!("sema-io: blocking offload failed: {e}"),
    }
}

/// Drive `fut` to completion ON THE CALLING THREAD using THE pool's reactor,
/// returning its output. `fut` may be non-`Send` and non-`'static`. Legal from
/// plain OS threads and `io_spawn_blocking` closures; PANICS from async worker
/// threads — see the threading contract in the module docs.
pub fn io_block_on<F: Future>(fut: F) -> F::Output {
    install();
    BLOCK_ON_OPS.fetch_add(1, Ordering::SeqCst);
    sema_core::io_block_on(fut)
}

/// How many times the pool builder has run (identity oracle; see
/// [`POOLS_BUILT`]).
pub fn pools_built() -> u64 {
    POOLS_BUILT.load(Ordering::SeqCst)
}

/// How many [`io_block_on`] entries have occurred (identity oracle; see
/// [`BLOCK_ON_OPS`]).
pub fn block_on_ops() -> u64 {
    BLOCK_ON_OPS.load(Ordering::SeqCst)
}
