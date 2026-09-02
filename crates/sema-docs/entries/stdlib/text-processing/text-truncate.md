---
name: "text/truncate"
module: "text-processing"
section: "Text Cleaning"
params: [{ name: text, type: string }, { name: max-length, type: integer }, { name: suffix, type: string, doc: "optional; defaults to \"...\"" }]
returns: "string"
see_also: ["text/excerpt", "string/truncate-width", "string/take"]
---

Truncate text to a maximum length with a suffix. Takes text, max-length, and optional suffix (default `"..."`).

```sema
(text/truncate "hello world" 5)       ; => "he..."
(text/truncate "hello world" 8 "…")   ; => "hello w…"
(text/truncate "hi" 10)               ; => "hi"
```
