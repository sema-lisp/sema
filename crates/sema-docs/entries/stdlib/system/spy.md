---
name: "spy"
module: "system"
section: "Debugging"
params: [{ name: label, type: string }, { name: value, type: any }]
returns: "any"
see_also: ["log/debug", "print", "time"]
---

Print `[label] value` to standard error and return `value` unchanged. Because it returns its second argument untouched, you can wrap any subexpression in `(spy "label" ...)` to peek at it without restructuring the surrounding code.

The label comes first, the value second, so `spy` wraps the value rather than threading through it — drop it around the expression you want to inspect.

```sema
(spy "x" (+ 1 2))   ; prints "[x] 3" to stderr, returns 3

;; Peek at nested intermediates without pulling them into lets
(+ 1 (spy "doubled" (* 2 (spy "in" 5))))
; prints "[in] 5" then "[doubled] 10" to stderr, returns 11
```

Output goes to stderr, so it stays out of a program's stdout/data output. See `time` to measure how long an expression takes instead of what it evaluates to.
