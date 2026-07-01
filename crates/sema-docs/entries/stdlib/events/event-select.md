---
name: "event/select"
module: "events"
section: "Events"
---

Poll a list of event sources and return the first that becomes ready, or `nil` on timeout. Sources are maps: `{:type :key}`, `{:type :proc :handle h}`, `{:type :timer :ms n}`. Optional second arg is a timeout in ms. The unified wait for a TUI loop.
