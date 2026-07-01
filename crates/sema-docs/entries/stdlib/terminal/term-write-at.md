---
name: "term/write-at"
module: "terminal"
section: "Screen Control"
syntax: "(term/write-at row col text)"
returns: "nil"
---

Move the cursor to a 1-based `row`/`col` and write `text` there in one operation. Combine with `term/style`/`term/rgb` for colored output.

```sema
(term/write-at 3 5 (term/rgb "status: ok" 200 168 85))
```
