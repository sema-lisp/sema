---
name: "term/enable-mouse"
module: "terminal"
section: "Screen Control"
---

Enable mouse reporting — button events, button-motion (drag), and SGR extended
coordinates — so the terminal sends click/drag/wheel events on stdin. `io/read-key`
decodes them into `{:kind :mouse :action … :x :y :button :mods}` (see `io/read-key`).
Pair with `term/disable-mouse`, or use the `term/with-mouse` guard to disable
automatically on exit. Takes no arguments.
