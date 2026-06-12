---
name: "llm/stream"
module: "llm"
params: [{ name: prompt }, { name: callback }, { name: opts, type: map }]
returns: "string"
---

Stream a completion from the default provider. The first argument is a prompt string, a prompt value, or a messages sequence. An optional function argument is invoked with each text chunk; without a callback, chunks are printed to stdout. An optional opts map accepts `:model`, `:max-tokens`, `:temperature`, and `:system`. Returns the full accumulated response string.

```sema
(llm/stream "Tell me a short story" (fn [chunk] (display chunk)) {:max-tokens 200})
```
