//! Filesystem watching (`fs/watch`, `fs/watch-events`, `fs/unwatch`).
//!
//! `fs/watch` registers a recursive/non-recursive watcher and returns an
//! integer handle. The OS delivers change events on a background thread into a
//! channel; `fs/watch-events` drains whatever has accumulated (non-blocking),
//! so a TUI can notice files changed outside the app on its own tick. The
//! watcher object is parked in a thread-local registry to keep it alive.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{channel, Receiver, Sender};

use notify::{Event, EventKind, RecursiveMode, Watcher};
use sema_core::{check_arity, Caps, SemaError, Value};

use crate::register_fn;

struct Watch {
    rx: Receiver<Event>,
    // Dropping this ends the background watcher thread (its `recv` errors),
    // which drops the watcher and stops watching — so removing the registry
    // entry (`fs/unwatch`) tears the watch down.
    _stop: Sender<()>,
}

thread_local! {
    static WATCHERS: RefCell<HashMap<i64, Watch>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<i64> = const { Cell::new(1) };
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

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "fs/watch", |args| {
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

        let (tx, rx) = channel();
        let (stop_tx, stop_rx) = channel::<()>();
        let path = path.to_string();
        // Establish the watch on a background thread: a recursive registration
        // over a large tree (or a filesystem root) can take a long time and must
        // never block the caller — e.g. `sema web`, which creates this watcher
        // before binding its HTTP server. The watcher lives on this thread and
        // is dropped (stopping the watch) once `stop_tx` in the registry entry
        // is dropped.
        std::thread::spawn(move || {
            let mut watcher =
                match notify::recommended_watcher(move |res: notify::Result<Event>| {
                    if let Ok(ev) = res {
                        let _ = tx.send(ev);
                    }
                }) {
                    Ok(w) => w,
                    Err(_) => return,
                };
            let _ = watcher.watch(std::path::Path::new(&path), mode);
            let _ = stop_rx.recv(); // park until the registry entry is dropped
        });

        let id = NEXT_ID.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        });
        WATCHERS.with(|w| w.borrow_mut().insert(id, Watch { rx, _stop: stop_tx }));
        Ok(Value::int(id))
    });

    // fs/watch-events — drain pending events: list of {:kind :paths}.
    register_fn(env, "fs/watch-events", |args| {
        check_arity!(args, "fs/watch-events", 1);
        let id = args[0].as_int().ok_or_else(|| {
            SemaError::type_error("integer (watcher handle)", args[0].type_name())
        })?;
        WATCHERS.with(|w| {
            let watchers = w.borrow();
            let watch = watchers
                .get(&id)
                .ok_or_else(|| SemaError::eval(format!("fs/watch-events: no such watcher {id}")))?;
            let mut events = Vec::new();
            while let Ok(ev) = watch.rx.try_recv() {
                let mut m = BTreeMap::new();
                m.insert(kw("kind"), kind_keyword(&ev.kind));
                let paths: Vec<Value> = ev
                    .paths
                    .iter()
                    .map(|p| Value::string(&p.to_string_lossy()))
                    .collect();
                m.insert(kw("paths"), Value::list(paths));
                events.push(Value::map(m));
            }
            Ok(Value::list(events))
        })
    });

    register_fn(env, "fs/unwatch", |args| {
        check_arity!(args, "fs/unwatch", 1);
        let id = args[0].as_int().ok_or_else(|| {
            SemaError::type_error("integer (watcher handle)", args[0].type_name())
        })?;
        WATCHERS.with(|w| {
            w.borrow_mut().remove(&id);
        });
        Ok(Value::nil())
    });
}
