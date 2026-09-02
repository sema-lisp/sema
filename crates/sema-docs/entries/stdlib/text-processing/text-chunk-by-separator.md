---
name: "text/chunk-by-separator"
module: "text-processing"
section: "Text Chunking"
params: [{ name: text, type: string }, { name: sep, type: string }]
returns: "list"
see_also: ["text/chunk", "string/split", "string/lines"]
---

Split text by a specific separator string.

```sema
(text/chunk-by-separator "a\nb\nc" "\n")  ; => ("a" "b" "c")
```
