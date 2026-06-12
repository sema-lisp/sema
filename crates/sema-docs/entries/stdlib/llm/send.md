---
name: "llm/send"
module: "llm"
params: [{ name: prompt, type: prompt }, { name: opts, type: map }]
returns: "string"
---

Send a structured prompt value (built with the prompt helpers) to the default provider and return the completion. The first argument must be a prompt value; the optional opts map carries model/generation options.

```sema
(llm/send (prompt (system "You are a helpful assistant.")
                  (user "Summarize the plot of Hamlet in one line."))
          {:max-tokens 80})
```
