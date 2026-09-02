---
name: "async/map"
module: "concurrency"
section: "Promises"
params: [{ name: f, type: function }, { name: items, type: "list | vector" }]
returns: "list"
see_also: ["async/pool-map", "async/spawn-all", "async/all", "pipeline"]
---

```sema
(async/map f items) → list
```

Concurrent map: apply `f` to each item in its **own** async task, returning the
results in **input order**. The unbounded sibling of
[`async/pool-map`](#async-pool-map) — every item gets a task at once, with no
concurrency cap. Use `async/pool-map` when you need to limit how many run together
(e.g. against a rate-limited API).

It's the ergonomic form of `(async/all (map (fn (x) (async/spawn (fn () (f x)))) items))`.

```sema
;; Fetch every URL concurrently.
(async/map (fn (u) (http/get u)) urls)

(async/map (fn (i) (* i i)) '(1 2 3 4))  ; => (1 4 9 16)
```
