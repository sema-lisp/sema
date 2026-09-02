---
name: "prompt/template"
module: "text-processing"
section: "Prompt Templates"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["prompt/render", "prompt/fill", "format"]
---

Create a template string for use with `prompt/render`.

```sema
(define tmpl (prompt/template "Hello {{name}}, welcome to {{place}}."))
```
