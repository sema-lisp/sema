---
name: "sys/term-size"
module: "system"
section: "Terminal"
---

Return the terminal's current size as a map `{:rows N :cols M}`, or `nil` when no controlling TTY is attached (e.g., when stdout is redirected to a file). Queries `ioctl(TIOCGWINSZ)` against stdout, then stderr, then stdin.

```sema
(sys/term-size)
;; => {:rows 47 :cols 180}
```

Pair with `sys/on-signal :winch` to redraw on terminal resize:

```sema
(define (redraw size)
  ;; ... layout for size ...
  )

(redraw (sys/term-size))
(sys/on-signal :winch (fn () (redraw (sys/term-size))))
```

Returns `nil` on Windows and any non-Unix target.
