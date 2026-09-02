---
name: "settled-partition"
module: "concurrency"
section: "Promises"
syntax: "(settled-partition results)"
returns: "map"
see_also: ["settled/ok?", "settled/err?", "parallel-settled", "pipeline-settled"]
---

Macro: split a list of settled `{:ok v}` / `{:err e}` results into a map with two keys:

- `:ok` — list of the unwrapped success values (inner `:ok` values in input order).
- `:err` — list of the unwrapped failure values (inner `:err` values in input order).

```sema
(settled-partition
  (list {:ok 1} {:err "boom"} {:ok 3}))
; => {:ok (1 3) :err ("boom")}

;; common pattern: fan-out, then partition
(define results
  (parallel-settled (list (fn () 1) (fn () (throw "x")) (fn () 3))))
(define parts (settled-partition results))
(:ok parts)   ; => (1 3)
(:err parts)  ; => (#<error "x">)
```

See also: `settled/ok?`, `settled/err?`, `parallel-settled`, `pipeline-settled`.
