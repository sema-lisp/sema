---
name: "checkpoint"
module: "workflow"
section: "Dynamic Workflows"
---

Record or read a keyed step value within a workflow run. `(checkpoint :k v)` stores `v` under key `:k`, emits a `checkpoint` event (with an opaque value digest), and returns `v` — so it threads naturally through a `let` or as a phase's last form. `(checkpoint :k)` reads the previously-stored value back (or `nil` if unset), letting a later phase consume what an earlier one produced. It doubles as the run-scoped state bag. Errors if called outside a `workflow/run`.

`phase` is a one-argument marker, so the `checkpoint` calls follow it as siblings (not
nested inside it):

```sema
(phase "Inventory")
(checkpoint :files (list "a.php" "b.php" "c.php"))   ; record + return

(phase "Audit")
(count (checkpoint :files))                          ; read back => 3
```

See also: `workflow/run`, `workflow/phase`.
