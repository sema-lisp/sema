---
name: "stream/read-all"
module: "streams"
section: "Reading"
params: [{ name: stream, type: stream }, { name: max-bytes, type: int, doc: "optional; caps the number of bytes read" }]
returns: "bytevector"
see_also: ["stream/read", "stream/read-line", "stream/to-string", "stream/copy"]
---

Read the stream into a bytevector, up to `max-bytes`. The optional limit
defaults to 256 MiB. The call fails before growing its result beyond that cap.

Use repeated `stream/read` calls when the input may be larger or does not have a
known end.

```sema
(define data (stream/read-all s (* 8 1024 1024))) ; 8 MiB maximum
(utf8->string data)    ; convert to string if text
```
