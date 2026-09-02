---
name: "settled/ok?"
module: "concurrency"
section: "Promises"
syntax: "(settled/ok? result)"
returns: "bool"
see_also: ["settled/err?", "settled-partition", "parallel-settled"]
---

Predicate: returns `#t` when `result` is a settled success — i.e. the map contains an
`:ok` key. A settled result is the `{:ok v}` / `{:err e}` shape produced per slot by
`parallel-settled`, `pipeline-settled`, and `with-retry`.

```sema
(settled/ok? {:ok 42})      ; => #t
(settled/ok? {:err "boom"}) ; => #f

;; filter successes out of a settled batch
(filter settled/ok? (parallel-settled (list (fn () 1) (fn () (throw "x")) (fn () 3))))
; => ({:ok 1} {:ok 3})
```

See also: `settled/err?`, `parallel-settled`, `pipeline-settled`, `settled-partition`.
