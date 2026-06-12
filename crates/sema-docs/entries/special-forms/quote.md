---
name: "quote"
module: "special-forms"
syntax: "(quote expr)"
---

`quote` returns its argument unevaluated. It prevents the evaluator from treating a list as a function call or a symbol as a variable reference. The reader shorthand `'` (apostrophe) desugars directly to `quote`, so `'x` is equivalent to `(quote x)`.

This form is fundamental for constructing data literals and for writing macros, because it lets you treat code as data.

```sema
(quote (1 2 3))
;; => (1 2 3)
```

```sema
'(+ 1 2)
;; => (+ 1 2)
```

```sema
'foo
;; => foo
```

```sema
'(a (b c) d)
;; => (a (b c) d)
```

**Note:** Quoting a symbol produces the symbol value itself, not the value bound to that name in the environment.
