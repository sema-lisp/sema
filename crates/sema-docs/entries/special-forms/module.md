---
name: "module"
module: "special-forms"
syntax: "(module name (export sym1 sym2 ...) body ...)"
---

Declare a module within a file. The first argument is the module name as a symbol. The second argument must be an export declaration of the form `(export sym1 sym2 ...)`, which lists the symbols that should be visible to code that imports this file. The remaining arguments are the module body expressions, evaluated in order.

Only names listed in the export clause are exposed to importers. Unexported names remain private to the module. The module system is used in conjunction with `import`, which loads a file containing one or more `module` declarations and selectively brings exported bindings into scope.

```sema
(module math
  (export square factorial)
  (define (square x) (* x x))
  (define (factorial n)
    (if (<= n 1)
      1
      (* n (factorial (- n 1)))))
  (define (helper x) (* x 2)))  ; private, not exported
```

A file can contain multiple module declarations, though typically one per file is used:

```sema
(module utils
  (export clamp lerp)
  (define (clamp x lo hi)
    (cond ((< x lo) lo) ((> x hi) hi) (else x)))
  (define (lerp a b t)
    (+ a (* t (- b a)))))
```

Importing a module with `import`:

```sema
(import "math.sema")
(square 5)     ; => 25
(factorial 5)  ; => 120
```

Selective import is supported by passing the desired symbols to `import`:

```sema
(import "math.sema" square)
```
