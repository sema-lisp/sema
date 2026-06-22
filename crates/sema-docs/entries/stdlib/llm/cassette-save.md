---
name: "llm/cassette-save"
module: "llm"
params: []
returns: "boolean"
---

Flush the active cassette's tape to disk. Returns `#t` if a cassette is active, `#f` otherwise. (The tape is also flushed automatically when an `llm/with-cassette` scope exits.)

```sema
(llm/cassette-save)
```
