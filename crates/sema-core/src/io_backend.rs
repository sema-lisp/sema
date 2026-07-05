//! Executor seam for offloaded I/O (ADR #69): one process-wide backend behind a
//! tokio-free trait, so every crate that offloads work (http, shell, LLM wire
//! calls) parks/wakes through ONE pool instead of growing its own runtime.
//!
//! Sixth instance of the house type-erased-registration idiom (precedents:
//! `set_eval_callback`, the otel task callbacks, the usage-scope callbacks,
//! `set_blocking_sleep_callback`, `set_interrupt_callback`). Two deliberate
//! divergences from those: the slot is a process-global `OnceLock` rather than a
//! thread-local (the backend is reachable from pool threads and plain OS threads
//! alike — precedent: `IO_SIGNAL` below in `async_signal`), and the three ops
//! share one trait object because they must share one pool identity.
//!
//! The blessed backend lives in the `sema-io` leaf crate. Consumer crates must
//! go through `sema_io::{io_spawn, io_spawn_blocking, io_block_on}` — the raw
//! functions here exist for `sema-io` itself and for a future wasm backend; a
//! source-conformance test (`runtime_conformance_test.rs`) forbids calling them
//! from anywhere else.
//!
//! # Threading contract (pinned by tokio-assumption tests in `sema-io`)
//!
//! `io_block_on` is legal from the VM thread, plain OS threads, and
//! `io_spawn_blocking` closures; it PANICS from `io_spawn` futures or any other
//! async-driver thread ("cannot block_on from within a runtime worker"). A
//! `block_on`'d future may transiently need at most ONE blocking slot of its own
//! (reqwest's GaiResolver DNS); never nest a second spawn_blocking-and-wait
//! level inside one — the pool's admission control reserves exactly depth-1
//! headroom.

use std::future::Future;
use std::pin::Pin;

/// One-shot cancel hook returned by [`io_spawn`] (a tokio `AbortHandle::abort`
/// on native; `AbortController.abort` on a future wasm backend). Slots into
/// `IoHandle::with_abort` one-for-one. Dropping the hook does NOT abort.
#[cfg(not(target_arch = "wasm32"))]
pub type AbortHook = Box<dyn FnOnce() + Send>;
/// One-shot cancel hook returned by [`io_spawn`] (wasm: single-threaded host,
/// so the hook need not be `Send`).
#[cfg(target_arch = "wasm32")]
pub type AbortHook = Box<dyn FnOnce()>;

/// The boxed future shape [`IoBackend::spawn`] accepts. `Send` on native (it
/// crosses onto pool threads); relaxed on wasm where everything is one thread.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxIoFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
/// The boxed future shape [`IoBackend::spawn`] accepts (wasm: non-`Send`).
#[cfg(target_arch = "wasm32")]
pub type BoxIoFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

/// The three executor operations every offload site needs, behind one backend
/// identity. Implemented over THE process-wide tokio pool by `sema-io` (native);
/// a future wasm backend implements the two spawn ops over fetch/JS promises and
/// panics in `block_on_boxed` (every synchronous blocking consumer is
/// `cfg(not(wasm32))`-gated).
pub trait IoBackend {
    /// Spawn a future on the pool; returns a one-shot abort hook.
    fn spawn(&self, fut: BoxIoFuture) -> AbortHook;

    /// Offload a synchronous closure to the pool's blocking tier.
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send>);

    /// Drive a boxed future to completion ON THE CALLING THREAD using the
    /// pool's reactor/timers. NATIVE-ONLY semantics: a wasm backend panics
    /// here. The non-`Send`, non-`'static` future shape is the point — provider
    /// `&self` borrows and streaming `on_chunk` callbacks over Sema values
    /// never leave the calling thread.
    fn block_on_boxed(&self, fut: Pin<Box<dyn Future<Output = ()> + '_>>);
}

#[cfg(not(target_arch = "wasm32"))]
static IO_BACKEND: std::sync::OnceLock<Box<dyn IoBackend + Send + Sync>> =
    std::sync::OnceLock::new();

/// Install the process-wide I/O backend. First-wins: returns `true` if this
/// call installed `backend`, `false` if one was already installed (the argument
/// is dropped). Idempotent by design — every entry point may call it.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_io_backend(backend: Box<dyn IoBackend + Send + Sync>) -> bool {
    IO_BACKEND.set(backend).is_ok()
}

/// The installed backend, if any.
#[cfg(not(target_arch = "wasm32"))]
pub fn io_backend() -> Option<&'static (dyn IoBackend + Send + Sync)> {
    IO_BACKEND.get().map(|b| b.as_ref())
}

// wasm32: single-threaded host — the slot is a thread-local holding a leaked
// `&'static` (one backend per process, installed once; leaking it is the
// cheapest way to hand out `&'static` without `Sync` machinery).
#[cfg(target_arch = "wasm32")]
thread_local! {
    static IO_BACKEND_TL: std::cell::Cell<Option<&'static dyn IoBackend>> =
        const { std::cell::Cell::new(None) };
}

/// Install the (thread-local on wasm) I/O backend. First-wins; see the native
/// variant.
#[cfg(target_arch = "wasm32")]
pub fn set_io_backend(backend: Box<dyn IoBackend>) -> bool {
    IO_BACKEND_TL.with(|slot| {
        if slot.get().is_some() {
            return false;
        }
        slot.set(Some(Box::leak(backend)));
        true
    })
}

/// The installed backend, if any.
#[cfg(target_arch = "wasm32")]
pub fn io_backend() -> Option<&'static dyn IoBackend> {
    IO_BACKEND_TL.with(|slot| slot.get())
}

#[cfg(not(target_arch = "wasm32"))]
fn require_backend() -> &'static (dyn IoBackend + Send + Sync) {
    io_backend().expect("no I/O backend installed — call sema_io::install() (or go through the sema_io::io_* wrappers, which install it)")
}

#[cfg(target_arch = "wasm32")]
fn require_backend() -> &'static dyn IoBackend {
    io_backend().expect("no I/O backend installed for this thread")
}

/// Raw seam entry: spawn a boxed future on the installed backend. Consumer
/// crates use `sema_io::io_spawn` (which installs the backend first); this raw
/// form panics when no backend is installed.
pub fn io_spawn(fut: BoxIoFuture) -> AbortHook {
    require_backend().spawn(fut)
}

/// Raw seam entry: offload a boxed closure to the backend's blocking tier.
/// Consumer crates use `sema_io::io_spawn_blocking`.
pub fn io_spawn_blocking(work: Box<dyn FnOnce() + Send>) {
    require_backend().spawn_blocking(work);
}

/// Raw seam entry: drive `fut` to completion on the CALLING thread using the
/// backend's reactor, returning its output. Generic sugar over
/// [`IoBackend::block_on_boxed`]: the output travels through a stack slot, so
/// `fut` may be non-`Send` and non-`'static` (provider `&self` borrows,
/// streaming callbacks over Sema values). Consumer crates use
/// `sema_io::io_block_on`. NATIVE-ONLY semantics — see the trait method.
pub fn io_block_on<F: Future>(fut: F) -> F::Output {
    let backend = require_backend();
    let mut slot = None;
    backend.block_on_boxed(Box::pin(async {
        slot = Some(fut.await);
    }));
    slot.expect("io_block_on: block_on_boxed returned without completing the future")
}
