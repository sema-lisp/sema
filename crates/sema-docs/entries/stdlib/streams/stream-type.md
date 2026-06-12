---
name: "stream/type"
module: "streams"
section: "Introspection"
---

Returns a string describing the stream implementation.

```sema
(stream/type (stream/byte-buffer))         ;; => "byte-buffer"
(stream/type (stream/from-string "x"))     ;; => "string"
(stream/type (stream/open-input "f.txt"))  ;; => "file-input"
(stream/type *stdout*)                     ;; => "stdout"
```
