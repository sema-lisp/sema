---
name: "assq"
module: "lists"
section: "Association Lists"
params: [{ name: obj, type: any }, { name: alist, type: list }]
returns: "list or #f"
see_also: ["assv", "assoc", "member", "list/find"]
---

Look up `obj` in an association list, a list whose elements are lists whose
first item is the key. Returns the first matching entry (the whole inner list),
or `#f` when no key matches. Entries that are not lists are skipped.

In R7RS `assq`, `assv`, and `assoc` differ only in the equality they use
(`eq?`, `eqv?`, `equal?`). Sema compares keys structurally in all three, so
they are interchangeable: a string or list key matches by value, not by
identity. The three names exist for Scheme compatibility.

The result is the entry itself, so take its second element for the value. An
entry written as a dotted pair `(a . 1)` is read as a three-element list
(`.` is a symbol), which is why `(b . 2)` prints back the same way.

```sema
(assq 'b '((a 1) (b 2)))        ; => (b 2)
(assq 'z '((a 1) (b 2)))        ; => #f
(assq "b" '(("a" 1) ("b" 2)))   ; => ("b" 2)
(cadr (assq 'b '((a 1) (b 2)))) ; => 2
```

For keyed data that is looked up often, a map (`{:a 1 :b 2}` with `get`) is
O(1) and usually clearer.
