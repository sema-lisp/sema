---
name: "pdf/metadata"
module: "pdf"
section: "Metadata"
---

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
