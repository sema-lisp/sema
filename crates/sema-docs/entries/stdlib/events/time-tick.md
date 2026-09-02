---
name: "time/tick"
module: "events"
section: "Events"
params: [{ name: ms, type: int }]
returns: "map"
see_also: ["event/select", "sleep"]
---

Build a reusable timer source for `event/select`. The argument `ms` is the interval in milliseconds; the timer fires once that many milliseconds have passed since the enclosing `event/select` call started, so passing the same source to each loop iteration gives a steady tick. The result is the plain map `{:type :timer :ms ms}`; it holds no state, so it can be built once and reused.

```sema
(time/tick 16)   ; => {:ms 16 :type :timer}
```
