---
name: "llm/with-fallback"
module: "llm"
params: [{ name: providers }, { name: thunk }]
returns: "any"
---

Run a zero-argument function with a fallback provider chain in effect. The first argument is a list of provider keywords/strings to try in order if the primary provider fails. The previous fallback chain is restored after the call. Returns the thunk's result.

```sema
(llm/with-fallback [:anthropic :openai :ollama]
  (fn [] (llm/complete "draft a one-line tagline")))
```
