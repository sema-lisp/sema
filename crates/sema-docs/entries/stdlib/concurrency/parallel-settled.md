---
name: "parallel-settled"
module: "concurrency"
section: "Promises"
syntax: "(parallel-settled thunks [n])"
returns: "list"
see_also: ["parallel", "pipeline-settled", "settled-partition", "settled/ok?"]
---

Macro: like `parallel`, but each slot carries the **raw settled result** —
`{:ok v}` on success or `{:err e}` on throw — rather than collapsing failures to `nil`.
Results come back in **input order**. The author chooses the failure policy (retry,
fallback, record, drop). Optional trailing `n` overrides the default concurrency cap (8).

Use `settled/ok?` / `settled/err?` / `settled-partition` to inspect or split the results.

```sema
(parallel-settled (list (fn () 1) (fn () (throw "boom")) (fn () 3)))
; => ({:ok 1} {:err #<error "boom">} {:ok 3})

;; keep only successes
(map (fn (r) (:ok r))
     (filter settled/ok? (parallel-settled thunks)))

;; with a custom concurrency cap
(parallel-settled heavy-thunks 4)
```

See also: `parallel`, `pipeline-settled`, `settled/ok?`, `settled/err?`, `settled-partition`, `with-retry`.
