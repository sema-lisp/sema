---
name: "context/set"
module: "context"
section: "Core Functions"
params: [{ name: key, type: keyword }, { name: value, type: any }]
returns: "nil"
see_also: ["context/get", "context/merge", "context/remove"]
---

Set a key-value pair in the current context frame.

The context is an ambient key-value store — a tidy alternative to threading the same arguments (trace ids, the current user, environment flags) through every function call. Anything written with `context/set` is readable anywhere downstream via `context/get`, until it is removed or the enclosing frame (see `context/with`) is popped.

```sema
(context/set :trace-id "abc-123")
(context/set :user-id 42)
(context/get :trace-id)            ; => "abc-123"
```

For values that should only exist for the duration of a call, prefer `context/with` (auto-cleaned scope) over a bare `context/set` you have to remember to `context/remove`. For secrets you don't want leaking into `context/all`, use `context/set-hidden`.
