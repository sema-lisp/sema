---
name: "llm/define-provider"
module: "llm"
params: [{ name: name, type: keyword }, { name: opts, type: map }]
returns: "keyword"
---

Define a custom LLM provider implemented in Sema. The map must include a `:complete` function (lambda or native fn) that receives a request map and returns a string or a response map (with `:content`, optional `:role`, `:model`, `:usage`, `:tool-calls`); `:default-model` is optional. Registers the provider and makes it the default, returning its name keyword.

```sema
(llm/define-provider :echo
  {:complete (fn [req] {:content (string-append "echo: " (:content (first (:messages req))))})
   :default-model "echo-1"})
```
