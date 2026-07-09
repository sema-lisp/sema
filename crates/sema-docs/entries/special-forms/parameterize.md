---
name: "parameterize"
module: "special-forms"
syntax: "(parameterize ((param val) ...) body ...)"
---

R7RS dynamic binding. `parameterize` evaluates each `val`, converts it through the corresponding parameter's converter (see `make-parameter`), installs the converted values, runs `body`, and always restores every parameter to its **prior** value before returning — even if `body` raises. The result is `body`'s last value (or the propagated condition, after restoration).

```sema
(define radix (make-parameter 10))

(radix)
;; => 10

(parameterize ((radix 16))
  (radix))
;; => 16

(radix)
;; => 10 (restored)
```

Conversion happens once, at install time; restoration sets the saved value back **raw** (it is never re-converted), so a non-idempotent converter cannot drift the parameter across repeated `parameterize` entries:

```sema
(define counter (make-parameter 0 (lambda (x) (+ x 1))))

(list (counter)
      (parameterize ((counter 10)) (counter))
      (counter))
;; => (1 11 1) — restore uses the saved raw 1, not (+ 1 1)
```

All bindings in a single `parameterize` form are converted **before** any of them are installed, so a converter that raises leaves every parameter untouched:

```sema
(define capped (make-parameter 1 (lambda (x) (if (> x 5) (error "too big") x))))

(guard (e (else :rejected))
  (parameterize ((capped 10)) (capped)))
;; => :rejected, and (capped) is still 1
```

Restoration runs on both the normal exit path and on a raised condition (the condition is re-raised after restoring), and `parameterize` forms nest — an inner `parameterize` restores back to the outer one's value, not the original:

```sema
(define mode (make-parameter :normal))

(parameterize ((mode :outer))
  (list (mode)
        (parameterize ((mode :inner)) (mode))
        (mode)))
;; => (:outer :inner :outer)
```

**Note:** `parameterize` is defined in the prelude (a macro over `try`/`catch`, mirroring `with-stream`'s cleanup idiom) and does not require an import. Because it must run its restore step after `body`, `body` is not in tail position.

See also: `make-parameter`.
