---
name: "gensym"
module: "system"
section: "Metaprogramming"
params: [{ name: prefix, type: string }]
returns: symbol
see_also: ["macroexpand", "str"]
---

Generate a fresh, unique symbol. The optional `prefix` (default `"g"`) is prepended to a global counter so each call yields a distinct name (`tmp__0`, `tmp__1`, ...) that cannot collide with anything in user code.

This is the standard fix for macro **variable capture**: when a macro introduces a temporary binding, hard-coding a name like `tmp` would clobber a caller's own `tmp`. Binding to a gensym instead guarantees the temporary is invisible to the expansion site.

```sema
(symbol? (gensym))      ; => #t
(= (gensym) (gensym))   ; => #f  (every call is distinct)

;; A swap macro that needs a temporary — gensym keeps it from
;; capturing a variable the caller might also be named.
(defmacro swap2 (a b)
  (let ((tmp (gensym "tmp")))
    (list 'let (list (list tmp a))
          (list 'set! a b)
          (list 'set! b tmp))))

(define x 1)
(define y 2)
(swap2 x y)
(list x y)   ; => (2 1)
```
