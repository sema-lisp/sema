---
name: "async/await"
module: "concurrency"
section: "Promises"
params: [{ name: promise, type: promise }]
returns: "any"
see_also: ["async/spawn", "async/all", "async/run", "async/timeout"]
---

```sema
(async/await promise) → value
```

Block on a promise and return its resolved value. Raises an error if the promise was rejected (or `"async/await: task was cancelled"` if it was cancelled). Usually written as the `await` special form.

Where you call it matters. **Inside** an async task, `await` *yields* to the scheduler — it suspends this task so other tasks can make progress, then resumes here once the promise settles (this is how concurrency happens: tasks interleave at `await`/`async/sleep`/channel points). **At the top level** (not inside a task), `await` drives the scheduler inline, running pending tasks until this promise settles.

Awaiting the same promise twice is fine — a promise caches its result, so the second `await` returns immediately without re-running the work.

```sema
(define p (async (+ 1 2)))
(await p)  ; => 3
(await p)  ; => 3  (cached, no re-run)

;; Concurrency: both tasks sleep at the same time, so this finishes in ~50ms,
;; not 100ms — the first await yields and lets the second task run.
(let ((a (async (async/sleep 50) :a))
      (b (async (async/sleep 50) :b)))
  (list (await a) (await b)))  ; => (:a :b)
```

See also: `async/all` to await many promises at once, `async/timeout` to bound the wait.
