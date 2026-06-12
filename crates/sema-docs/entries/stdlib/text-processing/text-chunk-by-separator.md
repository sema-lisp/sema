---
name: "text/chunk-by-separator"
module: "text-processing"
section: "Text Chunking"
---

Split text by a specific separator string.

```sema
(text/chunk-by-separator "a\nb\nc" "\n")  ; => ("a" "b" "c")
```
