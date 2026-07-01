---
name: "term/with-alt-screen"
module: "terminal"
section: "Screen Control"
---

Guard macro: enter the alternate screen and hide the cursor, run `body`, and
**always** restore both (show cursor, leave alt screen) on exit — even if `body`
throws (the error is re-raised after restoring). Returns `body`'s value. Prevents
a crash from leaving the terminal stuck in the alt buffer with a hidden cursor.
Compose with `io/with-raw-mode` / `term/with-mouse` (outermost restores last).

```sema
(io/with-raw-mode
  (term/with-alt-screen
    (term/with-mouse
      (run-tui))))   ; terminal is fully restored however this exits
```
