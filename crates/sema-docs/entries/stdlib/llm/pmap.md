---
name: "llm/pmap"
module: "llm"
params: [{ name: fn }, { name: collection }, { name: opts, type: map }]
returns: "list"
---

Map a function over a collection to produce prompt strings, then send all prompts to the default provider in parallel (via the provider's batch completion). Each item is passed to `fn` to build its prompt; the results are returned as a list of completion strings in input order. The opts map accepts `:model`, `:max-tokens`, `:temperature`, and `:system`.

```sema
(llm/pmap (fn [topic] (string-append "Define: " topic))
          ["entropy" "recursion" "monad"]
          {:max-tokens 60})
```
