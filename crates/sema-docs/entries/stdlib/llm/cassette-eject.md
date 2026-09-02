---
name: "llm/cassette-eject"
module: "llm"
params: []
returns: "boolean"
see_also: ["llm/cassette-load", "llm/cassette-save", "llm/with-cassette"]
syntax: "(llm/cassette-eject)"
---

Flush the active cassette's tape to disk and remove it, so subsequent LLM calls hit the real provider again. Returns `#t` if a cassette was active, `#f` otherwise.

```sema
(llm/cassette-eject)
```
