---
name: "macroexpand"
module: "special-forms"
syntax: "(macroexpand form)"
---

Evaluate the argument to obtain a form, then expand it one level if its head names a macro. Non-macro forms are returned unchanged. This is the primary tool for inspecting how macros rewrite code during development and debugging.

`macroexpand` first evaluates its argument (so you typically pass a quoted form), then checks whether the result is a list whose first element names a macro in the current environment. If so, it applies the macro once and returns the expanded result without further evaluating it. If the head is not a macro, or if the form is not a list, the form is returned as-is. Only a single level of expansion is performed — nested macro calls inside the expansion are not expanded.

```sema
(macroexpand '(when-let (x 1) x))
; => (let ((x 1)) (when (not (nil? x)) x))
```

```sema
(macroexpand '(-> 5 (+ 3) (* 2)))
; => (* (+ 5 3) 2)
```

```sema
(macroexpand '(+ 1 2))
; => (+ 1 2)   ; not a macro, returned unchanged
```

**Warning:** `macroexpand` evaluates its argument before expanding. If you pass an unquoted expression that evaluates to a macro call, it will expand that result. To inspect a literal form, always quote it.
