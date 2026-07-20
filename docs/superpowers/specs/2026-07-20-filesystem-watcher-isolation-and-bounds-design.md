# Filesystem watcher isolation and bounds

`fs/watch`, `fs/watch-events`, and `fs/unwatch` must operate on resources owned by the evaluator that registered the builtins. The current thread-local registry crosses that boundary: two interpreters on one thread share handles, queues, and teardown. A restricted interpreter can therefore guess another interpreter's handle, drain its events, or stop its watcher.

This slice replaces the ambient registry with evaluator-local ownership and bounds the resources under Sema's control. It does not claim that a platform watcher registration already blocked inside `notify` can be cancelled; that backend boundary remains nonterminal.

## Ownership model

Each call to `fs_watch::register` creates one `Rc<WatchRegistry>`. The three native functions installed in that environment capture the same registry, while independently constructed environments receive distinct registries. Handles are allocated only inside that registry, so the same integer may validly identify different watchers in different evaluators without granting cross-evaluator access.

The registry contains no `Value` or `Env`, satisfying the native-closure cycle invariant. `fs/watch` receives `EvalContext` through the context-aware native ABI. On the first live watcher epoch it registers one weak teardown hook with that context. Interpreter teardown upgrades the weak reference, stops every watcher, clears the registry, and resets the hook marker. A retained environment may later start a fresh epoch under another context without retaining the old context or its resources.

`fs/unwatch` removes only an entry from its captured registry. Removing an unknown handle remains idempotent for compatibility. `fs/watch-events` rejects an unknown handle in its captured registry even if the same number exists elsewhere.

## Sandbox boundary

`fs/watch` checks both `Caps::FS_READ` and `Sandbox::check_path` before inspecting the path, reserving capacity, or spawning a thread. Capability denial and path denial therefore have no watcher-side effects. The check uses the sandbox captured when the evaluator's stdlib was registered; it never consults ambient or process-global authority.

## Resource bounds

One registry admits at most 64 active or still-exiting watcher threads. Capacity belongs to a small thread lease and is released only when the background thread actually exits, not when its public handle is removed. Repeated watch/unwatch calls therefore cannot bypass the limit while slow registrations accumulate. Thread-spawn failure releases the reservation and returns an error.

Each watcher uses a bounded queue of 1,024 events. The `notify` callback never blocks: it uses `try_send`, increments a saturating dropped-event counter when the queue is full, and ignores delivery after teardown. `fs/watch-events` drains at most the bounded queue and appends one synthetic event after the retained events when drops occurred:

```sema
{:kind :overflow :paths () :dropped 37}
```

The overflow record reports loss since the previous drain. Consumers must rescan watched state rather than infer individual changes from that batch.

Dropping or explicitly removing a registry entry closes its stop channel. A watcher thread that has completed backend registration then drops its `notify` watcher and exits. Registry teardown itself does not join: it remains bounded even if the platform backend is stuck in construction or registration.

## Registration boundary

Watcher construction and `watcher.watch(...)` remain on the background thread because recursive registration can block. The public handle still represents a registration attempt, not proof that the operating system accepted the watch. Construction and registration errors cannot currently be returned synchronously without either blocking the evaluator or introducing an asynchronous readiness protocol.

A blocked platform registration cannot observe the stop channel until the backend call returns. The per-registry thread lease caps this failure mode, and interpreter teardown returns without waiting, but the host thread and platform call may outlive the interpreter. Fully terminal registration cancellation requires a killable helper process or a backend-specific interruptible API and is outside this slice.

## Regression coverage

Tests prove:

1. two evaluators receive isolated handle spaces; one cannot drain or stop the other's watcher;
2. a restricted evaluator rejects a real path outside its allowlist before spawning a watcher;
3. interpreter teardown stops all registered watcher entries even when the environment or native function remains retained;
4. removing a watcher releases its public handle but does not release thread capacity until the worker exits;
5. the active-thread limit rejects another watch without spawning work;
6. queue overflow is bounded, reports a deterministic dropped count, and resets that count after a drain;
7. watcher creation and registration failure do not panic, and thread-spawn failure rolls back capacity;
8. existing `fs/watch` event and `fs/unwatch` behavior remains green.
