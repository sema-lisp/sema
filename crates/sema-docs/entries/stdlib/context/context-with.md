---
name: "context/with"
module: "context"
section: "Scoped Overrides"
---

Push a temporary context frame for the duration of a thunk. The frame is automatically popped when the thunk completes — even if it raises an error.

```sema
(context/set :env "production")

(context/with {:env "staging" :debug #t}
  (lambda ()
    (context/get :env)      ; => "staging"
    (context/get :debug)))  ; => #t

(context/get :env)    ; => "production" (restored)
(context/get :debug)  ; => nil (gone)
```

Scopes nest naturally — inner values shadow outer ones:

```sema
(context/set :a 1)
(context/with {:b 2}
  (lambda ()
    (context/with {:c 3}
      (lambda ()
        (list (context/get :a) (context/get :b) (context/get :c))))))
; => (1 2 3)
```

Values set with `context/set` inside a `context/with` block are written to the inner frame and discarded when the scope exits. If you need a value to persist, set it before entering `context/with`.
