---
name: "export"
module: "special-forms"
syntax: "(export sym1 sym2 ...)"
---

Declare which symbols a module makes available to other files that import it. `export` is not a standalone special form — it only appears as the second argument inside a `module` form, specifying the public interface of that module.

Bindings listed in `export` become visible to importers. Any top-level definitions in the module body that are not exported remain private and cannot be accessed from outside the module. This is the primary mechanism for encapsulation in Sema's module system.

A file without a `module` form exports all of its top-level bindings by default. A `module` form always contains an `export` clause. Prefer an explicit module and export list when a file has a public API, so internal helpers do not become exports by accident.

```sema
;; math-utils.sema
(module math-utils
  (export square cube)
  (define (square x) (* x x))
  (define (cube x) (* x x x))
  (define (internal-helper x) x))       ; not exported
```

```sema
;; main.sema
(import "math-utils.sema")
(square 5)                              ; => 25
(cube 3)                                ; => 27
;; (internal-helper 1)                 ; error: unbound variable
```

```sema
;; Selective export with selective import
(module strings
  (export trim split)
  (define (trim s) ...)
  (define (split s sep) ...)
  (define (internal-escape s) ...))     ; private
```

**Note:** `export` takes zero or more bare symbols. An empty `(export)` clause makes all module definitions private. It has no effect outside of a `module` form. See `import` for how to consume exported bindings, and `module` for the full module declaration syntax.
