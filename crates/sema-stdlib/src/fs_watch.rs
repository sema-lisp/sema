//! Filesystem watching (`fs/watch`, `fs/watch-events`, `fs/unwatch`).
//!
//! `fs/watch` registers a recursive/non-recursive watcher and returns an
//! integer handle. The OS delivers change events on a background thread into a
//! channel; `fs/watch-events` drains whatever has accumulated (non-blocking),
//! so a TUI can notice files changed outside the app on its own tick. The
//! watcher object is parked in an evaluator-owned registry to keep it alive.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{
    channel, sync_channel, Receiver, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::Arc;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use sema_core::{check_arity, Caps, NativeFn, SemaError, Value};

use crate::register_fn;

const MAX_WATCH_WORKERS: usize = 64;
const MAX_WATCH_HANDLES: usize = 64;
const EVENT_QUEUE_CAPACITY: usize = 1_024;

struct Watch {
    rx: Receiver<Event>,
    dropped: Arc<AtomicUsize>,
    // Dropping this wakes the worker once platform construction or registration
    // returns. Those platform calls have no cancellation interface, so teardown
    // deliberately never joins the worker.
    _stop: Sender<()>,
}

impl Watch {
    fn drain_events(&self) -> Vec<Value> {
        let mut events = Vec::with_capacity(EVENT_QUEUE_CAPACITY + 1);
        for event in self.rx.try_iter().take(EVENT_QUEUE_CAPACITY) {
            events.push(event_value(&event));
        }
        let dropped = self.dropped.swap(0, Ordering::AcqRel);
        if dropped != 0 {
            events.push(overflow_value(dropped));
        }
        events
    }
}

struct EventSink {
    tx: SyncSender<Event>,
    dropped: Arc<AtomicUsize>,
}

impl EventSink {
    fn bounded(capacity: usize) -> (Self, Receiver<Event>, Arc<AtomicUsize>) {
        let (tx, rx) = sync_channel(capacity);
        let dropped = Arc::new(AtomicUsize::new(0));
        (
            Self {
                tx,
                dropped: Arc::clone(&dropped),
            },
            rx,
            dropped,
        )
    }

    fn send(&self, event: Event) {
        match self.tx.try_send(event) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                let _ = self
                    .dropped
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                        Some(count.saturating_add(1))
                    });
            }
        }
    }
}

struct WorkerCapacity {
    limit: usize,
    active: AtomicUsize,
}

impl WorkerCapacity {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            active: AtomicUsize::new(0),
        }
    }

    fn try_acquire(self: &Arc<Self>) -> Result<WorkerLease, SemaError> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.limit).then_some(active + 1)
            })
            .map_err(|_| {
                SemaError::eval(format!(
                    "fs/watch: too many live watcher workers (limit {})",
                    self.limit
                ))
            })?;
        Ok(WorkerLease {
            capacity: Arc::clone(self),
        })
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

struct WorkerLease {
    capacity: Arc<WorkerCapacity>,
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        let previous = self.capacity.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "worker lease count underflow");
    }
}

struct WatchRegistry {
    watchers: RefCell<HashMap<i64, Watch>>,
    next_id: Cell<i64>,
    capacity: Arc<WorkerCapacity>,
    teardown_hook_registered: Cell<bool>,
}

impl WatchRegistry {
    fn new() -> Self {
        Self::with_capacity(Arc::new(WorkerCapacity::new(MAX_WATCH_WORKERS)))
    }

    fn with_capacity(capacity: Arc<WorkerCapacity>) -> Self {
        Self {
            watchers: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
            capacity,
            teardown_hook_registered: Cell::new(false),
        }
    }

    fn insert(&self, watch: Watch) -> Result<i64, SemaError> {
        self.ensure_handle_capacity()?;
        let id = self.next_id.get();
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| SemaError::eval("fs/watch: watcher handle space exhausted"))?;
        self.watchers.borrow_mut().insert(id, watch);
        self.next_id.set(next_id);
        Ok(id)
    }

    fn ensure_handle_capacity(&self) -> Result<(), SemaError> {
        if self.watchers.borrow().len() >= MAX_WATCH_HANDLES {
            Err(SemaError::eval(format!(
                "fs/watch: too many live watcher handles (limit {MAX_WATCH_HANDLES})"
            )))
        } else {
            Ok(())
        }
    }

    fn with_available_handle<T>(
        &self,
        start: impl FnOnce() -> Result<T, SemaError>,
    ) -> Result<T, SemaError> {
        // The registry is evaluator-local (`Rc`) and all access is on the
        // evaluator thread, so this check remains valid until `start` returns
        // and the caller inserts its handle.
        self.ensure_handle_capacity()?;
        start()
    }

    fn remove(&self, id: i64) {
        self.watchers.borrow_mut().remove(&id);
    }

    fn drain_events(&self, id: i64) -> Result<Vec<Value>, SemaError> {
        let watchers = self.watchers.borrow();
        let watch = watchers
            .get(&id)
            .ok_or_else(|| SemaError::eval(format!("fs/watch-events: no such watcher {id}")))?;
        Ok(watch.drain_events())
    }

    fn ensure_teardown_hook(self: &Rc<Self>, ctx: &sema_core::EvalContext) {
        if !self.teardown_hook_registered.replace(true) {
            let registry = Rc::downgrade(self);
            ctx.register_interpreter_teardown_hook(move || {
                if let Some(registry) = registry.upgrade() {
                    registry.stop_all();
                }
            });
        }
    }

    fn stop_all(&self) {
        self.watchers.borrow_mut().clear();
        self.teardown_hook_registered.set(false);
    }
}

fn kw(s: &str) -> Value {
    Value::keyword(s)
}

fn kind_keyword(kind: &EventKind) -> Value {
    kw(match kind {
        EventKind::Create(_) => "create",
        EventKind::Modify(_) => "modify",
        EventKind::Remove(_) => "remove",
        EventKind::Access(_) => "access",
        _ => "other",
    })
}

fn event_value(event: &Event) -> Value {
    let mut map = BTreeMap::new();
    map.insert(kw("kind"), kind_keyword(&event.kind));
    let paths = event
        .paths
        .iter()
        .map(|path| Value::string(&path.to_string_lossy()))
        .collect();
    map.insert(kw("paths"), Value::list(paths));
    Value::map(map)
}

fn overflow_value(dropped: usize) -> Value {
    let mut map = BTreeMap::new();
    map.insert(kw("kind"), kw("overflow"));
    map.insert(kw("paths"), Value::list(Vec::new()));
    map.insert(
        kw("dropped"),
        Value::int(i64::try_from(dropped).unwrap_or(i64::MAX)),
    );
    Value::map(map)
}

fn stop_requested(stop_rx: &Receiver<()>) -> bool {
    matches!(stop_rx.try_recv(), Ok(()) | Err(TryRecvError::Disconnected))
}

trait WatchBackend {
    fn register(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()>;
}

impl WatchBackend for notify::RecommendedWatcher {
    fn register(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        Watcher::watch(self, path, mode)
    }
}

type WorkerJob = Box<dyn FnOnce() + Send>;

fn run_watch_worker<F, B>(
    lease: WorkerLease,
    sink: EventSink,
    stop_rx: Receiver<()>,
    path: String,
    mode: RecursiveMode,
    make_watcher: F,
) where
    F: FnOnce(EventSink) -> notify::Result<B>,
    B: WatchBackend,
{
    let _lease = lease;
    let mut watcher = match make_watcher(sink) {
        Ok(watcher) => watcher,
        Err(_) => return,
    };
    if stop_requested(&stop_rx) {
        return;
    }
    if watcher.register(Path::new(&path), mode).is_err() {
        return;
    }
    if !stop_requested(&stop_rx) {
        let _ = stop_rx.recv();
    }
}

fn spawn_watch_worker<S, F, B>(
    spawn: S,
    lease: WorkerLease,
    sink: EventSink,
    stop_rx: Receiver<()>,
    path: String,
    mode: RecursiveMode,
    make_watcher: F,
) -> std::io::Result<()>
where
    S: FnOnce(WorkerJob) -> std::io::Result<()>,
    F: FnOnce(EventSink) -> notify::Result<B> + Send + 'static,
    B: WatchBackend + 'static,
{
    spawn(Box::new(move || {
        run_watch_worker(lease, sink, stop_rx, path, mode, make_watcher);
    }))
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    let registry = Rc::new(WatchRegistry::new());
    let watch_registry = Rc::clone(&registry);
    let watch_sandbox = sandbox.clone();
    env.set(
        sema_core::intern("fs/watch"),
        Value::native_fn(NativeFn::with_ctx("fs/watch", move |ctx, args| {
            check_arity!(args, "fs/watch", 1..=2);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let recursive = args
                .get(1)
                .and_then(|o| o.as_map_ref())
                .and_then(|m| m.get(&kw("recursive")))
                .map(|v| v.is_truthy())
                .unwrap_or(true);

            watch_sandbox.check(Caps::FS_READ, "fs/watch")?;
            watch_sandbox.check_path(path, "fs/watch")?;

            // Surface the common error (bad path) synchronously; the actual
            // registration below runs off-thread and can't report back.
            if !std::path::Path::new(path).exists() {
                return Err(SemaError::Io(format!(
                    "fs/watch {path}: no such file or directory"
                )));
            }
            let mode = if recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };

            let path = path.to_string();
            let (rx, dropped, stop_tx) = watch_registry.with_available_handle(|| {
                let lease = watch_registry.capacity.try_acquire()?;
                let (sink, rx, dropped) = EventSink::bounded(EVENT_QUEUE_CAPACITY);
                let (stop_tx, stop_rx) = channel::<()>();
                // Establish the watch on a background thread: a recursive registration
                // over a large tree (or a filesystem root) can take a long time and must
                // never block the caller — e.g. `sema web`, which creates this watcher
                // before binding its HTTP server. A worker lease stays held until this
                // thread actually exits, including while a platform backend is stuck in
                // construction or registration.
                spawn_watch_worker(
                    |job| {
                        std::thread::Builder::new()
                            .name("sema-fs-watch".to_string())
                            .spawn(job)
                            .map(|_| ())
                    },
                    lease,
                    sink,
                    stop_rx,
                    path.clone(),
                    mode,
                    |sink| {
                        notify::recommended_watcher(move |result: notify::Result<Event>| {
                            if let Ok(event) = result {
                                sink.send(event);
                            }
                        })
                    },
                )
                .map_err(|error| {
                    SemaError::Io(format!("fs/watch {path}: failed to start watcher: {error}"))
                })?;
                Ok((rx, dropped, stop_tx))
            })?;

            let id = watch_registry.insert(Watch {
                rx,
                dropped,
                _stop: stop_tx,
            })?;
            watch_registry.ensure_teardown_hook(ctx);
            Ok(Value::int(id))
        })),
    );

    // fs/watch-events — drain pending events: list of {:kind :paths}.
    let event_registry = Rc::clone(&registry);
    register_fn(env, "fs/watch-events", move |args| {
        check_arity!(args, "fs/watch-events", 1);
        let id = args[0].as_int().ok_or_else(|| {
            SemaError::type_error("integer (watcher handle)", args[0].type_name())
        })?;
        Ok(Value::list(event_registry.drain_events(id)?))
    });

    let unwatch_registry = registry;
    register_fn(env, "fs/unwatch", move |args| {
        check_arity!(args, "fs/unwatch", 1);
        let id = args[0].as_int().ok_or_else(|| {
            SemaError::type_error("integer (watcher handle)", args[0].type_name())
        })?;
        unwatch_registry.remove(id);
        Ok(Value::nil())
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::{channel, sync_channel};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    struct FakeBackend {
        registration_error: Option<notify::Error>,
        ready: Option<Sender<()>>,
        event_sink: Option<EventSink>,
        dropped: Option<Sender<()>>,
    }

    impl WatchBackend for FakeBackend {
        fn register(&mut self, _path: &Path, _mode: RecursiveMode) -> notify::Result<()> {
            if let Some(ready) = self.ready.take() {
                let _ = ready.send(());
            }
            if let Some(sink) = &self.event_sink {
                sink.send(Event::new(EventKind::Any));
            }
            match self.registration_error.take() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    impl Drop for FakeBackend {
        fn drop(&mut self) {
            if let Some(dropped) = self.dropped.take() {
                let _ = dropped.send(());
            }
        }
    }

    #[test]
    fn fs_watch_worker_capacity_is_bounded_and_released_by_lease_drop() {
        let capacity = Arc::new(WorkerCapacity::new(MAX_WATCH_WORKERS));
        let mut leases = (0..MAX_WATCH_WORKERS)
            .map(|_| capacity.try_acquire().expect("slot available"))
            .collect::<Vec<_>>();

        assert_eq!(capacity.active(), MAX_WATCH_WORKERS);
        assert!(capacity.try_acquire().is_err());

        leases.pop();
        assert_eq!(capacity.active(), MAX_WATCH_WORKERS - 1);
        leases.push(capacity.try_acquire().expect("released slot available"));
        assert_eq!(capacity.active(), MAX_WATCH_WORKERS);
    }

    #[test]
    fn fs_watch_removing_handle_does_not_release_live_worker_lease() {
        let capacity = Arc::new(WorkerCapacity::new(MAX_WATCH_WORKERS));
        let lease = capacity.try_acquire().expect("slot available");
        let registry = WatchRegistry::with_capacity(Arc::clone(&capacity));
        let (stop_tx, stop_rx) = channel();
        let (release_tx, release_rx) = channel();
        let worker = std::thread::spawn(move || {
            let _lease = lease;
            let _ = stop_rx.recv();
            let _ = release_rx.recv();
        });
        let (_event_tx, event_rx) = sync_channel(EVENT_QUEUE_CAPACITY);
        registry.watchers.borrow_mut().insert(
            1,
            Watch {
                rx: event_rx,
                dropped: Arc::new(AtomicUsize::new(0)),
                _stop: stop_tx,
            },
        );

        registry.remove(1);
        assert_eq!(capacity.active(), 1);

        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(capacity.active(), 0);
    }

    #[test]
    fn fs_watch_event_sink_bounds_queue_and_reports_overflow_once() {
        let (sink, rx, dropped) = EventSink::bounded(EVENT_QUEUE_CAPACITY);
        for _ in 0..EVENT_QUEUE_CAPACITY + 17 {
            sink.send(Event::new(EventKind::Any));
        }
        let watch = Watch {
            rx,
            dropped: Arc::clone(&dropped),
            _stop: channel().0,
        };

        let batch = watch.drain_events();
        assert_eq!(batch.len(), EVENT_QUEUE_CAPACITY + 1);
        let overflow = batch
            .last()
            .and_then(Value::as_map_ref)
            .expect("overflow map");
        assert_eq!(overflow.get(&kw("kind")), Some(&kw("overflow")));
        assert_eq!(overflow.get(&kw("paths")), Some(&Value::list(Vec::new())));
        assert_eq!(overflow.get(&kw("dropped")), Some(&Value::int(17)));
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert!(watch.drain_events().is_empty());
    }

    #[test]
    fn fs_watch_construction_failure_releases_worker_lease() {
        let capacity = Arc::new(WorkerCapacity::new(1));
        let lease = capacity.try_acquire().expect("slot available");
        let (sink, _rx, _dropped) = EventSink::bounded(EVENT_QUEUE_CAPACITY);
        let (_stop_tx, stop_rx) = channel();

        spawn_watch_worker(
            |job| {
                job();
                Ok(())
            },
            lease,
            sink,
            stop_rx,
            "unused".to_string(),
            RecursiveMode::Recursive,
            |_sink| Err::<FakeBackend, _>(notify::Error::generic("construction failed")),
        )
        .unwrap();

        assert_eq!(capacity.active(), 0);
    }

    #[test]
    fn fs_watch_registration_failure_releases_worker_lease() {
        let capacity = Arc::new(WorkerCapacity::new(1));
        let lease = capacity.try_acquire().expect("slot available");
        let (sink, _rx, _dropped) = EventSink::bounded(EVENT_QUEUE_CAPACITY);
        let (_stop_tx, stop_rx) = channel();

        spawn_watch_worker(
            |job| {
                job();
                Ok(())
            },
            lease,
            sink,
            stop_rx,
            "unused".to_string(),
            RecursiveMode::Recursive,
            |_sink| {
                Ok(FakeBackend {
                    registration_error: Some(notify::Error::generic("registration failed")),
                    ready: None,
                    event_sink: None,
                    dropped: None,
                })
            },
        )
        .unwrap();

        assert_eq!(capacity.active(), 0);
    }

    #[test]
    fn fs_watch_spawn_failure_rolls_back_worker_lease() {
        let registry = WatchRegistry::new();
        let capacity = Arc::new(WorkerCapacity::new(1));
        let lease = capacity.try_acquire().expect("slot available");
        let (sink, _rx, _dropped) = EventSink::bounded(EVENT_QUEUE_CAPACITY);
        let (_stop_tx, stop_rx) = channel();

        let error = registry
            .with_available_handle(|| {
                spawn_watch_worker(
                    |_job| Err(std::io::Error::other("spawn failed")),
                    lease,
                    sink,
                    stop_rx,
                    "unused".to_string(),
                    RecursiveMode::Recursive,
                    |_sink| -> notify::Result<FakeBackend> {
                        unreachable!("failed spawn must not run the watcher factory")
                    },
                )
                .map_err(|error| SemaError::Io(error.to_string()))
            })
            .expect_err("spawn failure is returned");

        assert!(error.to_string().contains("spawn failed"));
        assert_eq!(capacity.active(), 0);
        assert!(registry.watchers.borrow().is_empty());
    }

    #[test]
    fn fs_watch_dead_handle_entries_are_bounded_and_unwatch_frees_capacity() {
        fn registration_failed_watch(registry: &WatchRegistry) -> Watch {
            let lease = registry
                .capacity
                .try_acquire()
                .expect("worker slot available");
            let (sink, rx, dropped) = EventSink::bounded(EVENT_QUEUE_CAPACITY);
            let (stop_tx, stop_rx) = channel();
            spawn_watch_worker(
                |job| {
                    job();
                    Ok(())
                },
                lease,
                sink,
                stop_rx,
                "unused".to_string(),
                RecursiveMode::Recursive,
                |_sink| {
                    Ok(FakeBackend {
                        registration_error: Some(notify::Error::generic("registration failed")),
                        ready: None,
                        event_sink: None,
                        dropped: None,
                    })
                },
            )
            .unwrap();
            assert_eq!(registry.capacity.active(), 0);
            Watch {
                rx,
                dropped,
                _stop: stop_tx,
            }
        }

        let registry = WatchRegistry::new();
        for expected_id in 1..=MAX_WATCH_HANDLES {
            let watch = registry
                .with_available_handle(|| Ok(registration_failed_watch(&registry)))
                .expect("handle slot available");
            assert_eq!(
                registry
                    .insert(watch)
                    .expect("checked handle slot remains available"),
                expected_id as i64
            );
        }
        let spawn_attempts = Cell::new(0);

        let error = registry
            .with_available_handle(|| {
                spawn_attempts.set(spawn_attempts.get() + 1);
                Ok(())
            })
            .expect_err("the sixty-fifth dead handle must be rejected");
        assert!(error.to_string().contains("too many live watcher handles"));
        assert_eq!(spawn_attempts.get(), 0, "rejection must not spawn a worker");
        assert_eq!(registry.watchers.borrow().len(), MAX_WATCH_HANDLES);

        registry.remove(1);
        registry
            .with_available_handle(|| {
                spawn_attempts.set(spawn_attempts.get() + 1);
                Ok(())
            })
            .expect("unwatch frees one public handle slot");
        assert_eq!(spawn_attempts.get(), 1);
        let watch = registry
            .with_available_handle(|| Ok(registration_failed_watch(&registry)))
            .expect("unwatch permits worker startup");
        assert_eq!(
            registry.insert(watch).expect("freed slot is reusable"),
            (MAX_WATCH_HANDLES + 1) as i64
        );
        assert_eq!(registry.watchers.borrow().len(), MAX_WATCH_HANDLES);
    }

    #[test]
    fn fs_watch_unwatch_tears_down_backend_and_releases_worker_lease() {
        let capacity = Arc::new(WorkerCapacity::new(1));
        let lease = capacity.try_acquire().expect("slot available");
        let registry = WatchRegistry::with_capacity(Arc::clone(&capacity));
        let (sink, rx, dropped) = EventSink::bounded(EVENT_QUEUE_CAPACITY);
        let (stop_tx, stop_rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let (backend_dropped_tx, backend_dropped_rx) = channel();
        let (worker_exited_tx, worker_exited_rx) = channel();

        spawn_watch_worker(
            move |job| {
                std::thread::Builder::new()
                    .name("fs-watch-unwatch-test".to_string())
                    .spawn(move || {
                        job();
                        let _ = worker_exited_tx.send(());
                    })
                    .map(|_| ())
            },
            lease,
            sink,
            stop_rx,
            "unused".to_string(),
            RecursiveMode::Recursive,
            move |sink| {
                Ok(FakeBackend {
                    registration_error: None,
                    ready: Some(ready_tx),
                    event_sink: Some(sink),
                    dropped: Some(backend_dropped_tx),
                })
            },
        )
        .unwrap();
        registry.watchers.borrow_mut().insert(
            1,
            Watch {
                rx,
                dropped,
                _stop: stop_tx,
            },
        );
        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("backend registered");

        registry.remove(1);

        backend_dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("backend dropped after unwatch");
        worker_exited_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker exited after unwatch");
        assert_eq!(capacity.active(), 0);
    }
}
