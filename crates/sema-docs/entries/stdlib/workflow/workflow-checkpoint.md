---
name: "workflow/checkpoint"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(workflow/checkpoint key [thunk])"
returns: "any"
see_also: ["checkpoint", "workflow/run", "workflow/phase"]
---

Record or read a keyed checkpoint in the active workflow context. With two
arguments, `(workflow/checkpoint key thunk)` records and returns `(thunk)`,
emits a `checkpoint` event with a `content_key`, and stores the canonical value
in the run memo sidecar. On `--resume`, a memoized checkpoint returns the stored
value before evaluating `thunk`, so expensive or side-effecting write thunks do
not rerun. With one argument, `(workflow/checkpoint key)` reads the value back
from the run-scoped state bag, returning `nil` if the key has not been written.
Usually written via the `checkpoint` macro, which wraps the write expression in
the thunk for you.

```sema
(workflow/checkpoint :files
  (fn () (list "a.php" "b.php" "c.php")))   ; record + return

(workflow/checkpoint :files)                ; read back
```

See also: `checkpoint`, `workflow/run`, `workflow/phase`.
