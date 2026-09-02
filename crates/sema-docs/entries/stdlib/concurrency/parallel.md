---
name: "parallel"
module: "concurrency"
section: "Promises"
syntax: "(parallel thunks [n])"
returns: "list"
see_also: ["parallel-settled", "pipeline", "async/pool-map", "async/spawn-all"]
---

Macro: run a list of zero-arg `thunks` concurrently and await them **all** before
returning — a barrier. Results come back in **input order**; a thunk that throws yields
`nil` in its slot rather than aborting the batch, so `(filter (fn (x) (not (nil? x)))
results)` drops the failures. Concurrency is bounded (default 8, or pass `n`) by the same
capacity-`n` semaphore that backs `pipeline`. This
mirrors the `parallel` combinator in Claude Code's `workflow.js`.

```sema
;; Both fetches run at once; results stay in order.
(parallel (list (fn () (http/get a)) (fn () (http/get b))))

(parallel (list (fn () 1) (fn () (throw "boom")) (fn () 3)))  ; => (1 nil 3)
```

See also: `pipeline`, `async/pool-map`, `async/spawn-all`.
