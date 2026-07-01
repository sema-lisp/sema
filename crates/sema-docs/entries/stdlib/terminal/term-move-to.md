---
name: "term/move-to"
module: "terminal"
section: "Screen Control"
syntax: "(term/move-to row col)"
returns: "nil"
---

Move the cursor to a 1-based `row` and `col`. Coordinates below 1 are clamped to 1.

```sema
(term/move-to 1 1)      ; top-left
(term/move-to 10 20)    ; row 10, column 20
```
