---
name: "term/bold"
module: "terminal"
section: "Modifiers"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["term/style", "term/strip", "term/dim"]
---

Render text in **bold** (increased intensity).

```sema
(term/bold "important")
(println (term/bold "Warning: check your input"))
```
