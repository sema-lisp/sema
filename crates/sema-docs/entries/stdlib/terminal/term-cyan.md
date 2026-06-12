---
name: "term/cyan"
module: "terminal"
section: "Colors"
params: [{ name: text, type: string }]
returns: "string"
---

Wrap `text` in ANSI escape codes so it renders in cyan in a terminal that supports color.

```sema
(term/cyan "hello")
```
