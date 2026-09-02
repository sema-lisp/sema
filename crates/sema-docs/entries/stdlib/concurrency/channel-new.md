---
name: "channel/new"
module: "concurrency"
section: "Channels"
params: [{ name: capacity, type: int, doc: "optional bound, defaults to 1; must be at least 1" }]
returns: "channel"
see_also: ["channel/send", "channel/recv", "channel/close", "channel?"]
---

```sema
(channel/new)         → channel  ; capacity 1
(channel/new capacity) → channel
```

Create a bounded, buffered channel for passing values between async tasks. Default capacity is 1; capacity must be at least 1. A sender that fills the buffer blocks (yields) until a receiver makes room, and a receiver on an empty channel blocks until a value arrives — this back-pressure is how producers and consumers stay in lock-step.

Channels are the message-passing counterpart to promises: use a promise for "compute one value and hand it back", a channel for "stream many values over time" (a work queue, a producer/consumer pipeline, a fan-in collector).

```sema
;; A producer feeds a bounded queue; the consumer drains it until close.
(define ch (channel/new 2))
(async (for-each (fn (n) (channel/send ch n)) '(1 2 3))
       (channel/close ch))
(define (drain acc)
  (let ((v (channel/recv ch)))
    (if (null? v) (reverse acc) (drain (cons v acc)))))
(await (async (drain '())))  ; => (1 2 3)
```
