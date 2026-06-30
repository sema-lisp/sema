---
name: "format/form"
module: "reflect"
section: "Reflection"
---

Pretty-print a form to canonical Sema source using the formatter. `(format/form (read/string "(define  x  1)"))` => `"(define x 1)"`.
