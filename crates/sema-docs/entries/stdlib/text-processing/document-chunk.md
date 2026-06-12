---
name: "document/chunk"
module: "text-processing"
section: "Documents"
---

Chunk a document, preserving and extending metadata. Each chunk gets `:chunk-index` and `:total-chunks` added to its metadata.

```sema
(document/chunk
  (document/create "long text..." {:source "paper.pdf"})
  {:size 500})
; => ({:text "chunk 1..." :metadata {:source "paper.pdf" :chunk-index 0 :total-chunks 3}}
;     {:text "chunk 2..." :metadata {:source "paper.pdf" :chunk-index 1 :total-chunks 3}}
;     ...)
```
