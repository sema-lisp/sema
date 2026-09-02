---
name: "term/strikethrough"
module: "terminal"
section: "Modifiers"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["term/style", "term/bold", "term/strip", "term/dim"]
---

Render text with a ~~strikethrough~~.

```sema
(term/strikethrough "deprecated")
```
