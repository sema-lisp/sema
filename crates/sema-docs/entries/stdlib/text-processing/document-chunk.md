---
name: "document/chunk"
module: "text-processing"
section: "Documents"
params: [{ name: doc, type: map }, { name: opts, type: map, doc: "optional; {:size 1000 :overlap 200}" }]
returns: "list"
see_also: ["document/create", "text/chunk", "document/text"]
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
