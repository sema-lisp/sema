---
name: "text/split-sentences"
module: "text-processing"
section: "Text Chunking"
params: [{ name: text, type: string }]
returns: "list"
see_also: ["text/chunk", "text/word-count", "string/words"]
---

Split text into sentences at `.`, `!`, `?` boundaries.

```sema
(text/split-sentences "Hello world. How are you? Fine.")
; => ("Hello world." "How are you?" "Fine.")
```
