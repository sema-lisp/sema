---
name: "pipeline-settled"
module: "concurrency"
section: "Promises"
syntax: "(pipeline-settled items stage ...)"
returns: "list"
see_also: ["pipeline", "parallel-settled", "settled-partition"]
---

Macro: like `pipeline`, but a stage that throws yields `{:err e}` for that item (instead of
`nil`), preserving the error. Items that survive every stage emerge as `{:ok final}`.
Results align to `items` in input order; nothing is dropped silently.

```sema
(pipeline-settled (list 0 1 2)
  (fn (i) (if (= i 1) (throw "boom") i))
  (fn (x) (* x 10)))
; => ({:ok 0} {:err #<error "boom">} {:ok 20})

;; extract surviving values
(map (fn (r) (:ok r))
     (filter ok? (pipeline-settled files audit-fn verify-fn)))
```

See also: `pipeline`, `parallel-settled`, `ok?`, `err?`, `settled-partition`.
