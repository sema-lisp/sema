---
name: "raise"
module: "system"
section: "Errors"
params: [{ name: obj, type: any }]
---

R7RS `raise`: raise an arbitrary object as an exception. Unlike `error` (which takes a message string and produces an `:eval` error), `raise` signals the object itself — any value, not just a string.

`raise` is a first-class procedure and is identical in effect to the `throw` special form: both build a `:user` exception. It exists so the raised object can be passed to `raise` from higher-order code (e.g. partially applied) where a special form cannot go.

```sema
(try (raise 42) (catch e (:value e)))     ; => 42
(try (raise {:code 404}) (catch e (:value e)))  ; => {:code 404}
```

`guard` recovers the raised object directly (it unwraps the `:user` wrapper), so clause tests read it raw:

```sema
(guard (e ((number? e) (* 2 e)) (else :other))
  (raise 21))
;; => 42
```

See `throw` (the special-form equivalent), `error` (message-based failures), and `guard` / `try` for handling.
