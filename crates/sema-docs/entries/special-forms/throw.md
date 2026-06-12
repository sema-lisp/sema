---
name: "throw"
module: "special-forms"
syntax: "(throw value)"
---

Raise a user exception with the given value. The value is evaluated and then wrapped in a `:user` error, which can be caught by `try` and its `catch` clause. When caught, the error map contains `:type` set to `:user`, `:message` set to the string representation of the value, and `:value` set to the original value. This makes `throw` the companion form to `try` for explicit error signaling.

```sema
(throw "something went wrong")
```

Any value can be thrown, not just strings. Maps and lists are useful for structured error information.

```sema
(throw {:code 404 :reason "not found"})
```

A thrown value is caught by `try` and appears in the catch variable under the `:value` key.

```sema
(try
  (throw {:status :error :detail "disk full"})
  (catch e
    (println (:message e))
    (:value e)))
; => {:status :error :detail "disk full"}
```

If a `throw` is not caught, the error propagates up the call stack until it is either caught by an outer `try` or causes the program to abort with the error message.

**Note:** Unlike many Lisp dialects where `throw` pairs with a catch tag, Sema's `throw` always works with `try`/`catch` in the style of modern language exception handling.
