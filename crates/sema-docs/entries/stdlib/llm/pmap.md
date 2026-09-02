---
name: "llm/pmap"
module: "llm"
params: [{ name: fn }, { name: collection }, { name: opts, type: map }]
returns: "list"
see_also: ["llm/batch", "llm/complete", "llm/with-rate-limit"]
---

Map a function over a collection to build prompt strings, then send them all to the default provider in parallel and return the completions in **input order**. Each item is passed to `fn`, which must return the prompt string for that item. The opts map accepts `:model`, `:max-tokens`, `:temperature`, and `:system`.

This is the "prompt template over data" helper: it's `llm/batch` fused with a `map`,
so instead of pre-building the list of prompts yourself you supply a function that
turns each input into its prompt. The model never sees `fn` — it runs locally to
produce text — and the calls are independent and concurrent, like `llm/batch`. Use
plain `llm/batch` when you already have a list of finished prompt strings.

```sema
;; One template, many inputs — three concurrent calls, results in order.
(llm/pmap (fn (topic) (string-append "Define in one line: " topic))
          ["entropy" "recursion" "monad"]
          {:max-tokens 60})
```
