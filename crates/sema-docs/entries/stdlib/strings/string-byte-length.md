---
name: "string/byte-length"
module: "strings"
section: "Unicode & Encoding"
---

Return the UTF-8 byte length of a string (as opposed to character count from `string/length`). Useful for understanding the actual memory footprint — emoji and CJK characters use more bytes than ASCII.

```sema
(string/byte-length "hello")   ; => 5   (ASCII: 1 byte each)
(string/byte-length "héllo")   ; => 6   (é is 2 bytes in UTF-8)
(string/byte-length "日本語")   ; => 9   (CJK: 3 bytes each)
(string/byte-length "😀")      ; => 4   (emoji: 4 bytes)
```

Compare with `string/length` which counts characters:

```sema
(string/length "😀")           ; => 1   (one character)
(string/byte-length "😀")      ; => 4   (four bytes)
```
