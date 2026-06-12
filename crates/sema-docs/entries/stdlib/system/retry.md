---
name: "retry"
module: "system"
section: "Control"
params: [{ name: thunk, type: function }, { name: opts, type: map }]
---

Call the zero-argument `thunk`, retrying with exponential backoff if it raises an error. Returns the first successful result, or re-raises the last error after all attempts fail. The optional `opts` map accepts `:max-attempts` (default 3), `:base-delay-ms` (default 100), and `:backoff` (default 2.0).

```sema
(retry (fn () (http/get "https://example.com")))
(retry flaky-thunk {:max-attempts 5 :base-delay-ms 50})
```
