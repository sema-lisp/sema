---
name: "document/metadata"
module: "text-processing"
section: "Documents"
params: [{ name: doc, type: map }]
returns: "map"
see_also: ["document/create", "document/text", "document/chunk"]
---

Extract the metadata from a document.

```sema
(document/metadata doc)  ; => {:source "test.txt" :page 1}
```
