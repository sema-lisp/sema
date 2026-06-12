---
name: "llm/summarize"
module: "llm"
params: [{ name: text, type: string }, { name: opts, type: map }]
returns: "string"
---

Summarize a block of text using the default provider. The opts map accepts `:model`, `:max-length` (target word count), and `:style` (`"paragraph"` default, `"bullet-points"`/`"bullets"`, or `"one-line"`). Returns the summary string.

```sema
(llm/summarize long-article {:style "bullet-points" :max-length 100})
```
