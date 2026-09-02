---
name: "workflow/check"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(workflow/check src)"
returns: "list"
see_also: ["workflow/run-form", "defworkflow", "defpolicy", "policy/without"]
---

Static-analyse a workflow source string (or any value, which is pretty-printed to source)
and return diagnostics as a list of maps — **no evaluation, no LLM calls, no I/O**. An
empty list means the source is clean. The checker validates literal `defpolicy`,
workflow `:policy`, step `:policy`, and `policy/without` forms in addition to workflow
shape and marker rules.

Each diagnostic map has the keys:

- `:severity` — `:error` or `:warning`.
- `:code` — a short code string (e.g. `"E-PHASE-ARITY"`).
- `:message` — human-readable description.
- `:line` — 1-based line number, or `nil` when no span is available.
- `:col` — 1-based column, or `nil` when no span is available.
- `:hint` — optional actionable guidance string, or `nil`.

```sema
;; check a source string
(workflow/check "(defworkflow bad \"doc\" {} (phase))")
; => ({:severity :error :code "E-PHASE-ARITY" :message "..." :line 1 :col 38 :hint "..."})

;; check a live form value (pretty-printed to source)
(define wf '(defworkflow ok "doc" {} (phase "Inventory") {:status :success}))
(workflow/check wf)   ; => ()

;; gate a self-rewrite loop: only run if clean
(let ((diags (workflow/check generated-src)))
  (if (null? diags)
    (workflow/run-form (read-many generated-src))
    (println (str "check failed: " (count diags) " issues"))))
```

See also: `workflow/run-form`, `defworkflow`, `defpolicy`, `policy/without`.
