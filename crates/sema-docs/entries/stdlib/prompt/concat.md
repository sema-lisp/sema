---
name: "prompt/concat"
module: "prompt"
params: [{ name: prompts, type: prompt }]
returns: "prompt"
---

Alias for `prompt/append`: concatenate one or more prompts into a single prompt, preserving message order.

```sema
(prompt/concat intro-prompt body-prompt closing-prompt)
```
