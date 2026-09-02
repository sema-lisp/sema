---
name: "stream/write-string"
module: "streams"
section: "Writing"
params: [{ name: stream, type: stream }, { name: s, type: string }]
returns: "int"
see_also: ["stream/write", "stream/read-line", "stream/flush"]
---

Write a string as UTF-8 bytes. Returns the number of bytes written.

```sema
(stream/write-string s "hello")   ;; => 5
```
