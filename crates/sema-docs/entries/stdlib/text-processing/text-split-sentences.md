---
name: "text/split-sentences"
module: "text-processing"
section: "Text Chunking"
---

Split text into sentences at `.`, `!`, `?` boundaries.

```sema
(text/split-sentences "Hello world. How are you? Fine.")
; => ("Hello world." "How are you?" "Fine.")
```
