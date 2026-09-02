---
name: "stream/write"
module: "streams"
section: "Writing"
params: [{ name: stream, type: stream }, { name: bytes, type: bytevector }]
returns: "int"
see_also: ["stream/write-string", "stream/write-byte", "stream/read", "stream/flush"]
---

Write the bytes of `bytes` to an output stream and return the number of bytes
written. The argument must be a bytevector; for text use `stream/write-string`,
which encodes a string as UTF-8, or convert first with `string->utf8`. A
single byte goes through `stream/write-byte`.

Output may be buffered by the stream; call `stream/flush` before another
process reads the data, or close the stream (`with-stream` does this for
you). Writing to an input-only stream is an error.

```sema
(define s (stream/byte-buffer))
(stream/write s (bytevector 72 105))     ; => 2
(stream/write s (string->utf8 "!"))      ; => 1
(stream/to-string s)                     ; => "Hi!"
```

```sema
;; Copy a file in fixed-size chunks.
(with-stream (in (stream/open-input "in.bin"))
  (with-stream (out (stream/open-output "out.bin"))
    (let loop ()
      (let ((chunk (stream/read in 65536)))
        (when chunk
          (stream/write out chunk)
          (loop))))))
```
