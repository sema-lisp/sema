---
name: "text/truncate"
module: "text-processing"
section: "Text Cleaning"
---

Truncate text to a maximum length with a suffix. Takes text, max-length, and optional suffix (default `"..."`).

```sema
(text/truncate "hello world" 5)       ; => "he..."
(text/truncate "hello world" 8 "…")   ; => "hello w…"
(text/truncate "hi" 10)               ; => "hi"
```
