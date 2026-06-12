---
name: "io/read-key-timeout"
module: "terminal"
section: "Raw-Mode Input"
---

Like `io/read-key`, but returns `nil` after `timeout-ms` milliseconds with no input. Backed by `select(2)`, so it doesn't burn CPU.

```sema
(io/read-key-timeout 100)   ; => key map, or nil after 100ms
```

Use this to drive an animation loop or to poll signals between renders:

```sema
(let loop ()
  (sys/check-signals)
  (let ((key (io/read-key-timeout 50)))
    (when key (handle-key key))
    (loop)))
```
