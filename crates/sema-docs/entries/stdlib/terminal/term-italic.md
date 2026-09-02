---
name: "term/italic"
module: "terminal"
section: "Modifiers"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["term/style", "term/bold", "term/strip", "term/dim"]
---

Render text in *italic*.

```sema
(term/italic "emphasis")
```
