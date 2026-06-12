---
name: "list/page"
module: "lists"
section: "Windowing"
---

Paginate a list. `(list/page items page per-page)` — 1-indexed pages.

```sema
(list/page (range 20) 1 5)   ; => (0 1 2 3 4)
(list/page (range 20) 2 5)   ; => (5 6 7 8 9)
```
