---
name: "term/underline"
module: "terminal"
section: "Modifiers"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["term/style", "term/bold", "term/strip", "term/dim"]
---

Render text with an underline.

```sema
(term/underline "click here")
```
