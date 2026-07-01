---
name: "string/to-utf8"
module: "bytevectors"
section: "String Conversion"
aliases: ["string->utf8", "string->bytevector"]
---

Encode a string as a UTF-8 bytevector — the raw bytes you'd write to a file or send over the network. ASCII characters map to one byte each; non-ASCII characters take two or more. Also available as `string->bytevector` (a Sema string encodes to its UTF-8 bytes).

```sema
(string/to-utf8 "hi")     ; => #u8(104 105)
(string/to-utf8 "é")      ; => #u8(195 169)  (one char, two bytes)
```

`utf8/to-string` is the inverse, so the round-trip is lossless:

```sema
(utf8/to-string (string/to-utf8 "héllo"))   ; => "héllo"
```
