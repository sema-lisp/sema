---
name: "async/pool-map"
module: "concurrency"
section: "Promises"
params: [{ name: f, type: function }, { name: items, type: "list | vector" }, { name: n, type: int }]
returns: "list"
see_also: ["async/map", "async/spawn-all", "parallel"]
---

```sema
(async/pool-map f items n) → list
```

Bounded-concurrency fan-out: apply `f` to each item with at most `n` tasks
running concurrently, returning the results in **input order**.

Unlike `(async/all (map #(async/spawn (fn () (f %))) items))`, which opens *all*
tasks at once, `async/pool-map` caps how many run simultaneously — so fanning out
over thousands of items won't open thousands of sockets or processes. Concurrency
is bounded by a semaphore (a capacity-`n` channel pre-filled with `n` tokens):
each task acquires a token before running `f` and releases it afterward on both
the success and error paths, so a throwing `f` can't deadlock the pool. A failing
item re-raises its error.

```sema
;; Fetch many URLs with at most 8 requests in flight at once.
(async/pool-map (fn (u) (http/get u)) urls 8)

(async/pool-map (fn (i) (* i i)) '(0 1 2 3 4 5) 2)  ; => (0 1 4 9 16 25)
```
