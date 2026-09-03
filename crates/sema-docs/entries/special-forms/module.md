---
name: "module"
module: "special-forms"
syntax: "(module name (export sym1 sym2 ...) body ...)"
---

Declare a module within a file. The first argument is the module name as a symbol. The second argument must be an export declaration of the form `(export sym1 sym2 ...)`, which lists the symbols that should be visible to code that imports this file. The remaining arguments are the module body expressions, evaluated in order.

Only names listed in the export clause are exposed to importers. Unexported names remain private to the module, but exported functions retain access to them. The module name describes the module; imported names are not automatically qualified with it.

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

Use one module declaration per file. A later declaration replaces the file's active export list; it does not create a second namespace. An empty `(export)` clause makes every binding private:

```sema
(module private-helpers
  (export)
  (define (helper x) (* x 2)))
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
