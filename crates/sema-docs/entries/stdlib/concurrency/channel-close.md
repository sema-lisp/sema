---
name: "channel/close"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "nil"
see_also: ["channel/new", "channel/closed?", "channel/recv", "channel/send"]
---

```sema
(channel/close ch)
```

Close the channel — the producer's way of saying "no more values". Already-buffered values stay receivable; once they're drained, every further `channel/recv` returns `nil` (the standard end-of-stream signal a consumer loops on), and any blocked receiver wakes with `nil`. A `channel/send` after close raises and drops the value.

Closing is the idiomatic way to end a producer/consumer loop: the producer closes when done, the consumer recvs until it sees `nil`.

```sema
(define ch (channel/new 4))
(channel/send ch 1)
(channel/close ch)
(list (channel/recv ch)     ; => 1   (buffered value still delivered)
      (channel/recv ch))    ; => nil (closed and empty)
```
