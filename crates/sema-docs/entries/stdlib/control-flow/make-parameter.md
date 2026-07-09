---
name: "make-parameter"
module: "control-flow"
params: [{ name: init, type: any }, { name: converter, type: procedure, doc: "optional; defaults to identity" }]
returns: "procedure"
see_also: ["parameterize"]
---

R7RS parameter object constructor. `(make-parameter init)` / `(make-parameter init converter)` returns a **parameter** — a zero-argument procedure that returns its current value. `converter`, if given, is applied to `init` immediately and to every value `parameterize` (or a direct mutating call) later installs; it runs exactly once per install, never on restore.

```sema
(define radix (make-parameter 10))

(radix)
;; => 10
```

```sema
(define scale (make-parameter 1 (lambda (x) (* x 2))))

(scale)
;; => 2 (converter already applied to init)
```

Calling a parameter with one argument mutates it directly (SRFI-39 style), converting the new value:

```sema
(define mode (make-parameter :normal))

(mode :debug)
(mode)
;; => :debug
```

For a *dynamically scoped* rebinding that restores automatically — including across a raised condition — use `parameterize` instead of mutating a parameter directly:

```sema
(parameterize ((radix 16))
  (radix))
;; => 16

(radix)
;; => 10 (restored)
```

**Note:** `make-parameter` is defined in the prelude and does not require an import. A parameter object is an ordinary closure over a mutable cell, so it is captured, passed, and garbage-collected like any other procedure.

See also: `parameterize`.
