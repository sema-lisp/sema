---
name: "with-retry"
module: "concurrency"
section: "Promises"
syntax: "(with-retry opts thunk)"
returns: "any"
see_also: ["parallel-settled", "pipeline-settled", "async/with-timeout"]
---

Macro: run a zero-arg `thunk` with bounded exponential backoff on failure. Returns the
thunk's value on success; re-raises the last error after all attempts are exhausted — so
it composes cleanly with `parallel-settled` / `pipeline-settled` (a exhausted `with-retry`
leaf surfaces as `{:err e}` in its slot).

`opts` keys (all optional):

- `:max` — total attempts (default `3`).
- `:base-ms` — first backoff delay in milliseconds (default `200`).
- `:factor` — delay multiplier per attempt (default `2`); delays are `base-ms`, `base-ms * factor`, `base-ms * factor^2`, …
- `:on` — `(fn (err attempt) …)` called after each failure, before the next sleep (for logging).

Each retry parks the current async task via `async/sleep` so sibling tasks continue.
Each attempt counts as a separate provider call for workflow budget purposes.

```sema
;; three attempts: 200ms, 400ms backoff, then re-raise
(with-retry {:max 3 :base-ms 200}
  (fn () (http/get "https://api.example.com/data")))

;; log failures, then give up
(with-retry {:max 5 :base-ms 100 :factor 1.5
             :on (fn (e n) (println (str "attempt " n " failed: " e)))}
  (fn () (llm/complete "Classify this text.")))

;; inside a fan-out — failed slots become {:err e}
(parallel-settled
  (map (fn (url) (fn () (with-retry {:max 2} (fn () (http/get url))))) urls))
```

See also: `parallel-settled`, `pipeline-settled`, `async/sleep`.
