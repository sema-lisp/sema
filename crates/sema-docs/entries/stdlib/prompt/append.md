---
name: "prompt/append"
module: "prompt"
params: [{ name: prompts, type: prompt }]
returns: "prompt"
---

Concatenate two or more prompts into a single prompt, preserving message order. Variadic; takes one or more prompt values.

```sema
(prompt/append system-prompt user-prompt)
```
