---
name: "print"
module: "file-io"
section: "Console I/O"
syntax: "(print value ...)"
returns: "nil"
---

Write values in read-syntax form (strings are quoted) like Scheme's `write`. No trailing newline. Use `display` for human-readable output without quotes.

```sema
(print "hello")   ;; outputs: "hello"
(display "hello") ;; outputs: hello
```
