---
name: "workflow/run-form"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(workflow/run-form form)"
---

Macro: evaluate a workflow form (or a list of top-level forms as returned by a
`{:schema :sema-form}` step) and return its `{:status …}` envelope.

- If `form` is a **list whose first element is itself a list** (i.e. a list of top-level
  forms), each form is evaluated in sequence via `eval` and the last result is returned —
  this is the idiomatic path for forms produced by `(step "…" {:schema :sema-form})`.
- Otherwise `form` is a single form (e.g. a `defworkflow` s-expression) and is evaluated
  directly.

This is the execution half of the self-rewrite loop: `step` produces the Sema source,
`workflow/check` validates it, `workflow/run-form` runs it.

```sema
;; run a single defworkflow form produced by an LLM step
(workflow/run-form
  (step "Generate a defworkflow for task X." {:schema :sema-form}))

;; run a list of top-level forms (multi-form file output)
(let ((forms (step "Generate a workflow file." {:schema :sema-form})))
  (when (null? (workflow/check forms))
    (workflow/run-form forms)))
```

See also: `workflow/check`, `defworkflow`, `step`.
