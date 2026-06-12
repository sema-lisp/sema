---
name: "list/join"
module: "lists"
section: "Padding & Joining"
---

Join list elements into a string. Optional final separator.

```sema
(list/join '(1 2 3) ", ")             ; => "1, 2, 3"
(list/join '(1 2 3) ", " " and ")     ; => "1, 2 and 3"
```
