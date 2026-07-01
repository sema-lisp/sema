---
outline: [2, 3]
---

# Reflection & Diagnostics

Parse, format, and check Sema source from Sema. Diagnostics come back as data,
which makes them ideal for agent repair loops.

```sema
(read/string "(+ 1 2)")            ; => the form (+ 1 2)
(read/all "(a) (b)")               ; => ((a) (b))
(format/form '(define  x  1))      ; => "(define x 1)"

(sema/check-string "(+ 1 2")       ; => {:ok #f :diagnostics [{:level :error
                                   ;        :code "syntax" :message ...
                                   ;        :span {:line :col :end-line :end-col}}]}
(sema/check-file "workflow.sema")  ; same, reading a file
```
