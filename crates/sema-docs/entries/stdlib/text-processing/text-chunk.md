---
name: "text/chunk"
module: "text-processing"
section: "Text Chunking"
---

Recursively split text into chunks, trying natural boundaries (paragraphs, sentences, words) before hard-splitting. Takes text and an optional options map.

```sema
(text/chunk "Long text here...")
(text/chunk "Long text here..." {:size 500 :overlap 100})
```

Options: `:size` (default 1000), `:overlap` (default 200). Returns a list of strings.
