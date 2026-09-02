---
name: "println"
module: "file-io"
section: "Console I/O"
syntax: "(println value ...)"
returns: "nil"
see_also: ["display", "print", "newline"]
---

Print a value in human-readable form (like `display`) followed by a newline. Strings are not quoted.

The everyday output function. It is `display` plus a `newline`: `(println "hi")` prints `hi` and moves to the next line. Reach for `print` instead only when you want re-readable, quoted output.

```sema
(println "with newline")   ; outputs: with newline\n
(println 42)               ; outputs: 42\n
(println (list 1 "a"))     ; outputs: (1 "a")\n
```
