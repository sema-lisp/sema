---
name: "term/red"
module: "terminal"
section: "Colors"
params: [{ name: text, type: string }]
returns: "string"
---

Wrap `text` in ANSI escape codes so it renders in red in a terminal that supports color.

```sema
(term/red "hello")
```
