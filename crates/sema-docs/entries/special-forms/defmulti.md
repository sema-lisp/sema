---
name: "defmulti"
module: "special-forms"
syntax: "(defmulti name dispatch-fn)"
---

Define a multimethod — a function whose implementation is selected at runtime by applying a dispatch function to the arguments. The `name` must be a symbol. The `dispatch-fn` is a regular function that receives the same arguments as the multimethod and returns a dispatch value. Methods are registered separately with `defmethod`, associating a dispatch value with a handler function.

Multimethods are open for extension: new methods can be added after the multimethod is defined, even from other modules. If no method matches the dispatch value and no `:default` method is registered, an error is raised. Dispatch values can be keywords, strings, numbers, lists, or any comparable value.

```sema
(defmulti area (fn (shape) (get shape :type)))
(defmethod area :circle (fn (s) (* 3.14159 (expt (get s :radius) 2))))
(defmethod area :rect   (fn (s) (* (get s :width) (get s :height))))

(area {:type :circle :radius 5})   ; => 78.53975
(area {:type :rect :width 3 :height 4})  ; => 12
```

Dispatch on runtime type:

```sema
(defmulti describe (fn (x) (type x)))
(defmethod describe :int    (fn (x) "integer"))
(defmethod describe :string (fn (x) "text"))
(defmethod describe :list   (fn (x) "list"))

(list (describe 42) (describe "hi") (describe '(1 2)))
;; => ("integer" "text" "list")
```

Multi-argument dispatch:

```sema
(defmulti combine (fn (a b) (list (type a) (type b))))
(defmethod combine '(:int :int)       (fn (a b) (+ a b)))
(defmethod combine '(:string :string) (fn (a b) (string-append a b)))

(combine 1 2)      ; => 3
(combine "a" "b")  ; => "ab"
```
