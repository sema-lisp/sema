---
name: "print"
module: "file-io"
section: "Console I/O"
---

Write values in read-syntax form (strings are quoted) like Scheme's `write`. No trailing newline. Use `display` for human-readable output without quotes.

```sema
(print "hello")   ;; outputs: "hello"
(display "hello") ;; outputs: hello
```
