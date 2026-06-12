---
name: "term/black"
module: "terminal"
section: "Colors"
params: [{ name: text, type: string }]
returns: "string"
---

Wrap `text` in ANSI escape codes so it renders in black in a terminal that supports color.

```sema
(term/black "hello")
```
