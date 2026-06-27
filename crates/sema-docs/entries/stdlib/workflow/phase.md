---
name: "phase"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(phase label)"
---

Macro: a journaled phase **marker** inside a workflow body (Claude Code `workflow.js`
semantics). `(phase label)` expands to `(workflow/phase label)`: it closes the
previously-open phase and opens `label`. Every `step`/`checkpoint` that follows belongs
to this phase until the next `(phase …)` marker or the run end. A phase is a journaling
boundary, not control flow — markers sit between the body's top-level forms rather than
wrapping them.

```sema
(phase "Inventory")
(checkpoint :files (list "a.php" "b.php" "c.php"))

(phase "Audit")
(checkpoint :findings (count (checkpoint :files)))
```

See also: `defworkflow`, `workflow/phase`, `checkpoint`.
