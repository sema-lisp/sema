---
name: "async/with-timeout"
module: "concurrency"
section: "Promises"
---

```sema
(async/with-timeout ms thunk) → value
```

Run a zero-argument **thunk** with a deadline: return its value if it settles within `ms` milliseconds, otherwise **cancel it** and raise a timeout condition. This is the *owned* (structured-concurrency) counterpart to `async/timeout`: where `async/timeout` observes an existing promise and leaves it running past the deadline, `async/with-timeout` owns the task it starts and tears it down when the deadline fires. A thunk that settles first has its value preserved.

```sema
(async/with-timeout 20 (fn () (async/sleep 1000) :never))
;; deadline wins: the child is cancelled and a :timeout condition is raised

(async/with-timeout 10000 (fn () (async/sleep 1) 42))  ; => 42
```

Durations are capped at `86_400_000` ms (1 day). Use `async/with-timeout` when you want the timed-out work stopped; use `async/timeout` when you only want to stop *waiting* but let the task continue.
