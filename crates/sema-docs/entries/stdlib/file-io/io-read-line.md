---
name: "io/read-line"
module: "file-io"
section: "Console I/O"
---

Read a line of input from stdin (trailing `\n` / `\r\n` stripped).

```sema
(define name (io/read-line))
```

Returns `nil` when stdin is closed (Ctrl-D in cooked mode, end of a piped file). Use this to distinguish "user pressed Enter on an empty line" (returns `""`) from "stdin is exhausted" (returns `nil`).

```sema
(let loop ()
  (let ((line (io/read-line)))
    (cond
      ((nil? line)         (println "(eof)"))
      ((= line "")         (loop))            ; blank line, keep reading
      (else                (println "got: " line) (loop)))))
```

Previously `io/read-line` returned `""` on both EOF and empty input, making them indistinguishable. It now returns `nil` on EOF. If you don't want to refactor for this, use `io/eof?` after the call instead.
