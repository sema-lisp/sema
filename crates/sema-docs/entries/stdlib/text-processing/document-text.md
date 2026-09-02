---
name: "document/text"
module: "text-processing"
section: "Documents"
params: [{ name: doc, type: map }]
returns: "string"
see_also: ["document/create", "document/metadata", "document/chunk"]
---

Extract the text from a document.

```sema
(document/text doc)  ; => "Hello world"
```
