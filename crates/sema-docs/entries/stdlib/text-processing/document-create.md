---
name: "document/create"
module: "text-processing"
section: "Documents"
---

Create a document map with `:text` and `:metadata`.

```sema
(document/create "Hello world" {:source "test.txt" :page 1})
; => {:metadata {:page 1 :source "test.txt"} :text "Hello world"}
```
