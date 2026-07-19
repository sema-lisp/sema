---
name: "file/fold-lines-bytes"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }, { name: f, type: function, doc: "(acc line-bytes) -> acc" }, { name: init, type: any }]
returns: "any"
---

Fold over the lines of a file with an accumulator, passing each line to the reducer as a **bytevector** (trailing `\n`/`\r\n` stripped, no UTF-8 validation). The byte-oriented sibling of `file/fold-lines`, for `bytes/*` pipelines that avoid per-line string decoding: `bytes/find` the separator, `bytes/parse-int10` the number, `bytes/->string` only what must become text.

Streams through a 64 KiB read buffer in bounded batches and never holds the whole file in memory. Each line may contain at most 256 KiB of content. A trailing `\n` or `\r\n` does not count toward the limit; a longer line raises an error.

Reach for `file/fold-lines` when you want lines as strings.

```sema
;; Sum one-decimal temperatures from "Name;-12.3" lines as ints ×10.
(file/fold-lines-bytes "measurements.txt"
  (fn (acc line)
    (let ((semi (bytes/find line 59)))          ; 59 = ';'
      (+ acc (bytes/parse-int10 line (+ semi 1)))))
  0)
```
