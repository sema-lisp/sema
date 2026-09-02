---
name: "llm/extract-from-image"
module: "llm"
params: [{ name: schema }, { name: source }, { name: opts, type: map }]
returns: "map"
see_also: ["llm/extract", "llm/complete"]
---

Extract structured data from an image into a value matching the given schema — the image counterpart of `llm/extract`. The source is either a file-path string or a bytevector of image bytes; the media type is auto-detected. The model (which must be vision-capable, e.g. `gpt-4o` or a Claude/Gemini vision model) is asked to return JSON only, which is parsed into a Sema value. The opts map accepts `:model`.

The schema works the same way as `llm/extract`: keys name the fields you want and
values describe their shape. This is the workhorse for receipts, invoices,
screenshots, and forms — describe the fields you need and let the model read the
pixels.

```sema
(llm/extract-from-image {:total :number :date :string} "receipt.png")
; => {:total 42.50 :date "2026-06-23"}
```
