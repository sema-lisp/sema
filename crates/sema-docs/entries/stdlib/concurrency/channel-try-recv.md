---
name: "channel/try-recv"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "any or nil"
see_also: ["channel/recv", "channel/empty?", "channel/count"]
---

```sema
(channel/try-recv ch) → value | nil
```

Non-blocking receive: return the next buffered value, or `nil` immediately if the channel is empty — it never yields or blocks. This is the polling counterpart to `channel/recv`, useful at the top level (where a blocking `recv` on an empty channel would raise) or to opportunistically drain whatever is ready without waiting.

A `nil` result can't be distinguished from a sent `nil`; pair with `channel/empty?` if you need to tell them apart, or send a sentinel value.

```sema
(define ch (channel/new 2))
(channel/send ch 5)
(list (channel/try-recv ch)    ; => 5
      (channel/try-recv ch))   ; => nil  (empty, did not block)
```
