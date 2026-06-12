---
name: "vector-store/add"
module: "vector-store"
params: [{ name: name, type: string }, { name: id, type: string }, { name: embedding, type: bytevector }, { name: metadata }]
returns: "string"
---

Add (or replace, by id) a document in the named vector store with its embedding bytevector and an arbitrary metadata value. Returns the document id.

```sema
(vector-store/add "docs" "doc-1" (llm/embed "hello") {:title "Greeting"})
```
