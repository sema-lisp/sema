---
name: "channel?"
module: "concurrency"
section: "Channels"
params: [{ name: value }]
returns: "bool"
---

`#t` if `value` is a channel (e.g. one created by `channel/new`), `#f` otherwise.

```sema
(channel? (channel/new 1))  ; => #t
(channel? 42)               ; => #f
```
