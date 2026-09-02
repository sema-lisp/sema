---
name: "term/inverse"
module: "terminal"
section: "Modifiers"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["term/style", "term/bold", "term/strip", "term/dim"]
---

Swap foreground and background colors.

```sema
(term/inverse "highlighted")
```
