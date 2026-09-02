---
name: "reverse"
module: "lists"
section: "Basic Operations"
params: [{ name: seq, type: "list | vector" }]
returns: "list or vector"
see_also: ["sort", "last", "append"]
---

Return a new sequence with the elements in the opposite order. The input is
not modified. A list comes back as a list and a vector as a vector; strings
are not sequences here, so use `string/reverse` for text.

`reverse` walks the whole input once, so it is O(n). It is often the last
step of an accumulator loop that builds a list by consing onto the front,
which produces the elements back to front.

```sema
(reverse '(1 2 3))    ; => (3 2 1)
(reverse [1 2 3])     ; => [3 2 1]
(reverse '())         ; => ()
```

```sema
;; Build a list front-to-back with cons, then fix the order once.
(define (squares n)
  (let loop ((i 1) (acc '()))
    (if (> i n)
        (reverse acc)
        (loop (+ i 1) (cons (* i i) acc)))))
(squares 4)   ; => (1 4 9 16)
```
