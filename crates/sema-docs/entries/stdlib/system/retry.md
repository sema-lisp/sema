---
name: "retry"
module: "system"
section: "Control"
params: [{ name: thunk, type: function }, { name: opts, type: map }]
returns: "any"
see_also: ["try", "error", "sleep"]
---

Call the zero-argument `thunk`, retrying with exponential backoff if it raises an error. Returns the first successful result, or re-raises the **last** error after all attempts fail. The optional `opts` map accepts `:max-attempts` (default 3), `:base-delay-ms` (default 100), and `:backoff` (default 2.0).

The delay before the Nth retry is `base-delay-ms * backoff^(N-1)`, so the defaults sleep ~100ms then ~200ms between three attempts. No delay follows the final attempt, and a `base-delay-ms` of 0 retries immediately with no sleep.

Gotchas: `retry` catches **every** error, including bugs and non-transient failures — there is no retry predicate, so a thunk that always throws simply runs `max-attempts` times before re-raising. The backoff is fixed (no random jitter), so many concurrent retriers can stampede in lockstep; stagger them yourself if that matters. Put only the failure-prone operation in the thunk — anything before it re-runs on every attempt.

```sema
;; Retry a flaky network call up to 5 times, 50ms/100ms/200ms/400ms backoff
(retry (fn () (http/get "https://example.com"))
       {:max-attempts 5 :base-delay-ms 50})

;; Succeed on the third try: a counter that throws twice
(define n 0)
(retry (fn ()
         (set! n (+ n 1))
         (if (< n 3) (error "not yet") n))
       {:base-delay-ms 0})
; => 3
```

See `error` for what gets raised, and `try`/`catch` if you want to handle the failure instead of retrying.
