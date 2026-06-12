---
name: "stream/read-all"
module: "streams"
section: "Reading"
---

Read the entire stream into a bytevector.

```sema
(define data (stream/read-all s))
(utf8->string data)    ; convert to string if text
```
