---
name: "prompt/render"
module: "text-processing"
section: "Prompt Templates"
---

Render a template by substituting `{{key}}` placeholders with values from a map. Missing keys are left as-is.

```sema
(prompt/render "Hello {{name}}, welcome to {{place}}."
  {:name "Alice" :place "Wonderland"})
; => "Hello Alice, welcome to Wonderland."

(prompt/render "Hello {{name}}, {{missing}}." {:name "Bob"})
; => "Hello Bob, {{missing}}."

;; Non-string values are stringified
(prompt/render "Count: {{n}}" {:n 42})
; => "Count: 42"
```
