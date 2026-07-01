---
name: "utf8/to-string"
module: "bytevectors"
section: "String Conversion"
aliases: ["utf8->string", "bytevector->string"]
---

Decode a bytevector of UTF-8 bytes back into a string. This is the inverse of `string/to-utf8` — use it to turn bytes read from a file or socket into text. Also available as `bytevector->string`.

```sema
(utf8/to-string #u8(104 105))       ; => "hi"
(utf8/to-string #u8(195 169))       ; => "é"  (two bytes, one char)
```
