---
name: "term/blue"
module: "terminal"
section: "Colors"
params: [{ name: text, type: string }]
returns: "string"
---

Wrap `text` in ANSI escape codes so it renders in blue in a terminal that supports color.

```sema
(term/blue "hello")
```
