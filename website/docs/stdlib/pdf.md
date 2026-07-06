---
outline: [2, 3]
---

# PDF Processing

Pure-Rust PDF text extraction, page counting, and metadata reading. No external tools required — works cross-platform including macOS, Linux, and Windows.

::: tip
These functions use the `pdf-extract` and `lopdf` Rust crates internally. They work with text-based PDFs. For scanned/image-only PDFs, consider using [`llm/extract-from-image`](../llm/extraction) with vision models instead.
:::

## Text Extraction

### `pdf/extract-text`

Extract all text from a PDF file, concatenated across all pages.

```sema
(pdf/extract-text "invoice.pdf")
; => "Invoice\nDate: 2025-01-15\nAmount: $50.00 USD\n..."

;; Clean up whitespace for LLM processing
(text/clean-whitespace (pdf/extract-text "invoice.pdf"))
; => "Invoice Date: 2025-01-15 Amount: $50.00 USD ..."
```

### `pdf/extract-text-pages`

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

## Metadata

### `pdf/page-count`

Return the number of pages in a PDF.

```sema
(pdf/page-count "report.pdf")
; => 12
```

### `pdf/metadata`

Return a map of PDF metadata fields. Always includes `:pages`; other fields (`:title`, `:author`, `:subject`, `:creator`, `:producer`) are included when present in the PDF.

```sema
(pprint (pdf/metadata "document.pdf"))
; => {:author "John Doe"
;     :creator "LibreOffice Writer"
;     :pages 5
;     :producer "LibreOffice"
;     :title "Quarterly Report"}

;; Access individual fields
(get (pdf/metadata "document.pdf") :title)
; => "Quarterly Report"

(get (pdf/metadata "document.pdf") :pages)
; => 5
```

## Example: Receipt Processor

Combine PDF extraction with [LLM structured extraction](../llm/extraction) to build an intelligent document processor:

```sema
;; Extract text from a PDF invoice
(define text (text/clean-whitespace (pdf/extract-text "invoice.pdf")))
(define pages (pdf/page-count "invoice.pdf"))
(println (format "Extracted ~a chars from ~a page(s)" (string/length text) pages))

;; Use LLM to classify and extract structured data
(llm/auto-configure)
(define result
  (llm/extract
    {:isReceipt {:type :boolean :description "Is this a receipt or invoice?"}
     :vendor {:type :string :description "The seller/merchant name"}
     :amount {:type :string :description "Total amount with currency"}
     :date {:type :string :description "Invoice date in YYYY-MM-DD format"}}
    text))

(println (format "Vendor: ~a" (get result :vendor)))
(println (format "Amount: ~a" (get result :amount)))
```

See the full [GLaDOS receipt processor example](https://github.com/sema-lisp/sema/blob/main/examples/glados-downloads.sema) for a complete implementation.
