---
name: "pdf/extract-text-pages"
module: "pdf"
section: "Text Extraction"
---

Extract text from a PDF, returning a list of strings — one per page.

```sema
(pdf/extract-text-pages "report.pdf")
; => ("Page 1 content..." "Page 2 content..." "Page 3 content...")

;; Get text from a specific page
(nth (pdf/extract-text-pages "report.pdf") 0)
; => "Page 1 content..."

;; Process each page separately
(for-each
  (fn (page-text)
    (println (format "Page has ~a words" (text/word-count page-text))))
  (pdf/extract-text-pages "report.pdf"))
```
