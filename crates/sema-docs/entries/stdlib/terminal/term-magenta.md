---
name: "term/magenta"
module: "terminal"
section: "Colors"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["term/style", "term/rgb", "term/strip"]
---

Wrap `text` in ANSI escape codes so it renders in magenta in a terminal that supports color.

```sema
(term/magenta "hello")
```
