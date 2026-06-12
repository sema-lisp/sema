---
name: "file/fold-lines"
module: "file-io"
section: "File Operations"
---

Fold over lines of a file with an accumulator. Uses a 256KB buffer for high throughput on large files.

```sema
(file/fold-lines "data.csv"
  (fn (acc line) (+ acc 1))
  0)
; => number of lines
```
