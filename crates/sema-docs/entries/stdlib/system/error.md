---
name: "error"
module: "system"
section: "Errors"
params: [{ name: message, type: any }]
returns: "never returns; raises an error"
see_also: ["raise", "throw", "try", "assert"]
---

Raise an error (a catchable exception) with the given message. Non-string arguments are stringified.

An error caught by `try`/`catch` is a map: it carries `:message` (your text) and `:type` `:eval`, so a handler can inspect or re-throw it. Use `error` for domain failures you expect callers to handle; use `assert` when a violated invariant should be a hard programming error.

```sema
(try (error "boom") (catch e (get e :message)))   ; => "boom"

(try (error 404) (catch e e))
; => {:message "404" :type :eval}

;; Validate, then fail loudly with a useful message
(define (withdraw balance amount)
  (if (> amount balance)
      (error f"insufficient funds: ${amount} > ${balance}")
      (- balance amount)))

(try (withdraw 100 250) (catch e (get e :message)))
; => "insufficient funds: 250 > 100"
```

See `assert` / `assert=` for test-style checks, and `retry` for retrying a thunk that raises.
