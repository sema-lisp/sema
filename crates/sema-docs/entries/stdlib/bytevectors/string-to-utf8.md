---
name: "string/to-utf8"
module: "bytevectors"
section: "String Conversion"
aliases: ["string->utf8"]
---

Encode a string as a UTF-8 bytevector.

```sema
(string/to-utf8 "hi")     ; => #u8(104 105)
(string/to-utf8 "Hello")  ; => #u8(72 101 108 108 111)
```
