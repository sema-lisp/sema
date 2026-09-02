---
name: "text/chunk"
module: "text-processing"
section: "Text Chunking"
params: [{ name: text, type: string }, { name: opts, type: map, doc: "optional; {:size 1000 :overlap 200}" }]
returns: "list"
see_also: ["text/chunk-by-separator", "document/chunk", "text/split-sentences"]
---

Recursively split text into chunks, trying natural boundaries (paragraphs, sentences, words) before hard-splitting. Takes text and an optional options map.

```sema
(text/chunk "Long text here...")
(text/chunk "Long text here..." {:size 500 :overlap 100})
```

Options: `:size` (default 1000), `:overlap` (default 200). Returns a list of strings.
