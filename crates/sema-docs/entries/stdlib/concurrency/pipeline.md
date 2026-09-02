---
name: "pipeline"
module: "concurrency"
section: "Promises"
syntax: "(pipeline items stage ...)"
returns: "list"
see_also: ["pipeline-settled", "parallel", "async/map"]
---

Macro: thread each of `items` through **all** stage functions independently — with **no
barrier between stages**. Every item is its own task, so item A can be in stage 3 while
item B is still in stage 1; wall-clock is the slowest single-item chain, not the sum of
per-stage maxima. Each stage fn receives the previous stage's result. A stage that throws
drops **that** item to `nil` (and skips its remaining stages) without aborting the batch;
results align to `items` (nils for dropped). Concurrency is bounded by the same semaphore
as `parallel`. This mirrors the `pipeline`
combinator in Claude Code's `workflow.js`.

```sema
;; Each file is audited then verified; the two stages overlap across files.
(pipeline files
  (fn (f) (agent (str "Audit " f) {:schema finding}))
  (fn (x) (agent (str "Verify " (:claim x)) {:schema verdict})))

(pipeline (list 0 1 2)
  (fn (i) (if (= i 1) (throw "boom") i))
  (fn (x) (* x 10)))                           ; => (0 nil 20)
```

See also: `parallel`, `async/pool-map`, `agent`.
