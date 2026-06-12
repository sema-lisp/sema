---
name: "term/strip"
module: "terminal"
section: "Stripping ANSI Codes"
---

Remove all ANSI escape sequences from a string, returning plain text.

```sema
(term/strip (term/bold "hello"))         ; => "hello"
(term/strip (term/style "hi" :red :bold)) ; => "hi"
(term/strip (term/rgb "color" 255 0 0))  ; => "color"
(term/strip "no codes here")            ; => "no codes here"
```

This is useful when you need plain text for logging to files, comparisons, or passing to functions that don't understand ANSI codes:

```sema
;; Write clean text to a file, styled text to terminal
(define msg (term/green "Build succeeded"))
(println msg)                          ; styled on terminal
(file/write "build.log" (term/strip msg))  ; clean in log file
```
