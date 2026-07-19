---
name: "file/fold-lines"
module: "file-io"
section: "File Operations"
---

Fold over the lines of a file with an accumulator, streaming one line at a time. The reducer receives `(acc line)` — accumulator first, then the current line (newline already stripped) — and returns the next accumulator.

This is the memory-efficient way to summarize a large file: it reads through a 64 KiB buffer in bounded batches instead of materializing every line as a list, as `(foldl f init (file/read-lines path))` does. Each line may contain at most 256 KiB of content. A trailing `\n` or `\r\n` does not count toward the limit; a longer line raises an error.

Reach for `file/for-each-line` instead when you only need side effects and no running result.

```sema
;; Count lines.
(file/fold-lines "data.csv" (fn (acc line) (+ acc 1)) 0)
; => 42

;; Sum the first CSV column without loading the file.
(file/fold-lines "nums.csv"
  (fn (total line) (+ total (string->number (first (string/split line ",")))))
  0)
```
