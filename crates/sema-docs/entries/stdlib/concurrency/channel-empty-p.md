---
name: "channel/empty?"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "bool"
see_also: ["channel/full?", "channel/count", "channel/try-recv"]
---

`#t` if the channel's buffer currently holds no items, `#f` otherwise. Errors if the argument is not a channel.

```sema
(channel/empty? (channel/new 1))  ; => #t
```
