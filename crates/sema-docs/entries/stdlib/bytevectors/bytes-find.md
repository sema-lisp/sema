---
name: "bytes/find"
module: "bytevectors"
section: "Byte Ops"
params: [{ name: bv, type: bytevector, doc: "haystack" }, { name: needle, type: any, doc: "a byte (int 0-255), bytevector, or string" }, { name: start, type: int, doc: "search from this byte offset", optional: true }]
returns: "int or nil"
see_also: ["bytes/slice", "bytes/ref", "bytes/length"]
---

Find the first occurrence of a needle in a bytevector: a memchr-style byte search. Returns the absolute byte index, or `nil` when absent. The needle is a single byte (int 0–255), a bytevector, or a string (searched as its UTF-8 bytes). The optional `start` offset resumes a scan without slicing.

```sema
(bytes/find (string->utf8 "Oslo;-12.3") 59)      ; => 4 (the ';' byte)
(bytes/find (string->utf8 "hello") "llo")        ; => 2
(bytes/find (string->utf8 "a;b;c") 59 2)         ; => 3
(bytes/find (string->utf8 "abc") 59)             ; => nil
```
