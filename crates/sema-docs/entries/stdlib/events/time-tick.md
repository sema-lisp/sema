---
name: "time/tick"
module: "events"
section: "Events"
---

Build a reusable timer source for `event/select`: `(time/tick 16)` fires every ~16ms. Returns `{:type :timer :ms 16}`.
