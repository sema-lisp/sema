---
name: "pdf/page-count"
module: "pdf"
section: "Metadata"
params: [{ name: path, type: string }]
returns: "int"
see_also: ["pdf/metadata", "pdf/extract-text-pages"]
---

Return the number of pages in a PDF.

```sema
(pdf/page-count "report.pdf")
; => 12
```
