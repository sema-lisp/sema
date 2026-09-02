---
name: "stream/byte-buffer"
module: "streams"
section: "Creating Streams"
params: []
returns: "stream"
see_also: ["stream/to-bytes", "stream/to-string", "stream/from-bytes", "stream/write-string"]
syntax: "(stream/byte-buffer)"
---

Create a read/write in-memory buffer — the workhorse for assembling output without touching the filesystem. Writes append to the buffer; reads consume from a separate read cursor.

`stream/to-string` and `stream/to-bytes` snapshot the *whole* accumulated buffer (they do not consume), which is the usual way to get the result out. `stream/read`/`stream/read-byte` instead advance a read cursor through the bytes you've written, so a buffer can act as both sink and source.

```sema
;; Accumulate text, then snapshot it
(let ((buf (stream/byte-buffer)))
  (stream/write-string buf "hello ")
  (stream/write-string buf "world")
  (stream/to-string buf))   ;; => "hello world"

;; Write then read back through the cursor
(let ((buf (stream/byte-buffer)))
  (stream/write buf (bytevector 1 2 3))
  (stream/read-byte buf))   ;; => 1
```

See also `stream/from-string`/`stream/from-bytes` for read-only sources, and `stream/to-string`/`stream/to-bytes` to extract the contents.
