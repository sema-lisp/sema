---
name: "pdf/extract-text"
module: "pdf"
section: "Text Extraction"
---

Extract all text from a PDF file, concatenated across all pages.

```sema
(pdf/extract-text "invoice.pdf")
; => "Invoice\nDate: 2025-01-15\nAmount: $50.00 USD\n..."

;; Clean up whitespace for LLM processing
(text/clean-whitespace (pdf/extract-text "invoice.pdf"))
; => "Invoice Date: 2025-01-15 Amount: $50.00 USD ..."
```
