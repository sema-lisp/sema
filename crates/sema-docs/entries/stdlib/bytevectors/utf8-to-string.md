---
name: "utf8/to-string"
module: "bytevectors"
section: "String Conversion"
aliases: ["utf8->string"]
---

Decode a bytevector as a UTF-8 string.

```sema
(utf8/to-string #u8(104 105))       ; => "hi"
(utf8/to-string #u8(72 101 108))    ; => "Hel"
```
