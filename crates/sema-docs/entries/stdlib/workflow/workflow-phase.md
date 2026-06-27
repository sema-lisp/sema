---
name: "workflow/phase"
module: "workflow"
section: "Dynamic Workflows"
---

Open a journaled phase **marker** inside a workflow body (not a wrapper, not control
flow). `(workflow/phase label)` closes the previously-open phase — emitting its
`phase.ended` — then emits `phase.started` for `label`; the checkpoints and steps that
follow attribute to this phase until the next `(workflow/phase …)` or the run end (which
`workflow/run` closes automatically). Returns `nil`. Usually written via the
`phase` macro.

```sema
(phase "Inventory")
(checkpoint :files (list "a.php" "b.php" "c.php"))

(phase "Audit")                       ; closes "Inventory", opens "Audit"
(checkpoint :findings (count (checkpoint :files)))
```

See also: `phase`, `workflow/run`, `checkpoint`.
