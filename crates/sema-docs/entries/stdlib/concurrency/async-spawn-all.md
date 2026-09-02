---
name: "async/spawn-all"
module: "concurrency"
section: "Promises"
params: [{ name: thunks, type: "list | vector" }]
returns: "list"
see_also: ["async/spawn", "async/all", "async/map", "parallel"]
---

```sema
(async/spawn-all thunks) → list
```

Spawn a list of zero-argument functions as concurrent tasks and await them all,
returning the results in **input order**. The ergonomic form of the very common
`(async/all (map (fn (th) (async/spawn th)) thunks))`.

Unbounded — every thunk gets its own task at once; use
[`async/pool-map`](#async-pool-map) to cap how many run concurrently.

```sema
;; Run two independent fetches at the same time.
(async/spawn-all (list (fn () (http/get url-a))
                       (fn () (http/get url-b))))

(async/spawn-all (list (fn () :a) (fn () :b) (fn () :c)))  ; => (:a :b :c)
```
