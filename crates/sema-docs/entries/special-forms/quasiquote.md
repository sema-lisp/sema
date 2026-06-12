---
name: "quasiquote"
module: "special-forms"
syntax: "(quasiquote expr)"
---

`quasiquote` creates a data template with selective evaluation. Like `quote`, it prevents evaluation of the overall form, but inside a quasiquote you can use `,expr` (unquote) to evaluate individual expressions and insert their results. You can also use `,@expr` (unquote-splicing) to evaluate an expression that must produce a list or vector, and splice each of its elements into the surrounding sequence.

Quasiquote works with lists, vectors, and maps. It also supports auto-gensym: any symbol ending with `#` (but not `##`) is replaced by a unique generated symbol that is consistent across all occurrences within the same quasiquote form. This is invaluable for writing hygienic macros.

The reader shorthand `` ` `` expands to `quasiquote`, so `` `(a ,b) `` is equivalent to `(quasiquote (a (unquote b)))`.

```sema
(define x 42)
`(a b ,x)
;; => (a b 42)
```

```sema
`(a ,@(list 1 2 3) b)
;; => (a 1 2 3 b)
```

```sema
(defmacro my-let1 (val body)
  `(let ((x# ,val)) ,body))
(let ((x 10))
  (my-let1 42 x))
;; => 10
```

**Note:** Unquote-splicing can only be used inside sequences (lists or vectors). Splicing into a map is not supported.
