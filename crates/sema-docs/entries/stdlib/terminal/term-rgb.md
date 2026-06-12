---
name: "term/rgb"
module: "terminal"
section: "True Color"
---

Apply 24-bit true color to text. Takes the text followed by red, green, and blue values (integers 0–255).

```sema
(term/rgb "orange" 255 165 0)
(term/rgb "coral" 255 127 80)
(term/rgb "teal" 0 128 128)
(term/rgb "hot pink" 255 105 180)
```

Uses the `ESC[38;2;r;g;bm` escape sequence format, which is supported by most modern terminals.

```sema
;; Build a gradient
(for-each
  (lambda (i)
    (display (term/rgb "█" (* i 25) 50 (- 255 (* i 25)))))
  (range 11))
(println)
```
