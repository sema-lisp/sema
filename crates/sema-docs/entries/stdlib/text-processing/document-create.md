---
name: "document/create"
module: "text-processing"
section: "Documents"
params: [{ name: text, type: string }, { name: metadata, type: map }]
returns: "map"
see_also: ["document/text", "document/metadata", "document/chunk"]
---

Create a document map with `:text` and `:metadata`.

```sema
(document/create "Hello world" {:source "test.txt" :page 1})
; => {:metadata {:page 1 :source "test.txt"} :text "Hello world"}
```
