---
name: "llm/extract-from-image"
module: "llm"
params: [{ name: schema }, { name: source }, { name: opts, type: map }]
returns: "map"
---

Extract structured data from an image into a value matching the given schema. The source is either a file-path string or a bytevector of image bytes; the media type is auto-detected. The model is asked to return JSON only, which is parsed into a Sema value. The opts map accepts `:model`.

```sema
(llm/extract-from-image {:total :number :date :string} "receipt.png")
```
