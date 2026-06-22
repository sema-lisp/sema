---
name: "llm/cassette-eject"
module: "llm"
params: []
returns: "boolean"
---

Flush the active cassette's tape to disk and remove it, so subsequent LLM calls hit the real provider again. Returns `#t` if a cassette was active, `#f` otherwise.

```sema
(llm/cassette-eject)
```
