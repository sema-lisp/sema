---
name: "cdr"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: list }]
returns: "list"
---

Return the rest of a list (everything after the first element).

```sema
(cdr '(1 2 3))     ; => (2 3)
(cdr '(1))         ; => ()
```

`car` and `cdr` are inherited from the [IBM 704](http://bitsavers.informatik.uni-stuttgart.de/pdf/ibm/704/24-6661-2_704_Manual_1955.pdf) (1955), the machine Lisp was originally implemented on. The 704 stored cons cells in a single 36-bit word, with two 15-bit pointer fields: the **address** field (bits 21-35) pointed to the first element, and the **decrement** field (bits 3-17) pointed to the rest of the list. `car` stands for "Contents of the Address Register" and `cdr` for "Contents of the Decrement Register" — they were single hardware instructions that extracted these sub-fields. Sema also provides `first`/`rest` as more readable aliases.
