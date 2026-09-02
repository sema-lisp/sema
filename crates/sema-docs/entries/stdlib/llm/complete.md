---
name: "llm/complete"
module: "llm"
params: [{ name: prompt, type: string }, { name: opts, type: map }]
returns: "string"
see_also: ["llm/chat", "llm/stream", "llm/extract", "llm/send"]
---

Send a single prompt to the default provider and return the completion text. The first argument is a prompt string (or a prompt value). The optional opts map accepts `:model`, `:max-tokens` (defaults to 4096), `:temperature`, and `:system`. Tracks token usage.

This is the simplest LLM call — one stateless prompt in, one string out, no
conversation history. Reach for `llm/chat` when you need multi-turn context or
tools, `llm/stream` to receive the reply incrementally, and `llm/extract` when you
want a structured value instead of free text. After a call, `llm/last-usage` reports
the tokens (and cost, if pricing is known) it consumed.

```sema
(llm/complete "Write a haiku about autumn" {:max-tokens 100 :temperature 0.7})

;; A :system prompt steers tone/role without becoming part of the user text.
(llm/complete "Explain recursion."
              {:system "You answer in one short sentence." :temperature 0.0})
```
