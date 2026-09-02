---
name: "settled/err?"
module: "concurrency"
section: "Promises"
syntax: "(settled/err? result)"
returns: "bool"
see_also: ["settled/ok?", "settled-partition", "parallel-settled"]
---

Predicate: returns `#t` when `result` is a settled failure — i.e. the map contains an
`:err` key. A settled result is the `{:ok v}` / `{:err e}` shape produced per slot by
`parallel-settled`, `pipeline-settled`, and `with-retry`.

```sema
(settled/err? {:err "boom"}) ; => #t
(settled/err? {:ok 42})      ; => #f

;; extract failure reasons from a settled batch
(map (fn (r) (:err r))
     (filter settled/err? (parallel-settled (list (fn () 1) (fn () (throw "x"))))))
; => (#<error "x">)
```

See also: `settled/ok?`, `parallel-settled`, `pipeline-settled`, `settled-partition`.
