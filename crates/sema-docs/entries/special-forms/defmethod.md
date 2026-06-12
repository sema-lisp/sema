---
name: "defmethod"
module: "special-forms"
syntax: "(defmethod multi-name dispatch-value handler-fn)"
---

Register a method implementation on an existing multimethod defined with `defmulti`. When the multimethod is called and its dispatch function returns a value equal to `dispatch-value`, the `handler-fn` is invoked with the original arguments. The `multi-name` must be a symbol naming an already-defined multimethod.

Use the special dispatch value `:default` to register a fallback handler that runs when no other method matches. Methods can be added at any time after the multimethod is created, including conditionally or from other modules, making the system open for extension.

`defmethod` returns `nil`.

```sema
(defmulti area (fn (shape) (get shape :type)))
(defmethod area :circle (fn (s) (* 3 (get s :radius) (get s :radius))))
(defmethod area :rect   (fn (s) (* (get s :width) (get s :height))))
(defmethod area :default (fn (s) 0))

(area {:type :circle :radius 5})  ; => 75
(area {:type :square})             ; => 0 (default method)
```

Adding methods dynamically after the multimethod is already in use:

```sema
(defmulti greet (fn (x) (get x :lang)))
(defmethod greet :en (fn (x) "hello"))

(greet {:lang :en})   ; => "hello"

(defmethod greet :fr (fn (x) "bonjour"))
(greet {:lang :fr})   ; => "bonjour"
```

Dispatch on integer values:

```sema
(defmulti fizzbuzz (fn (n)
  (cond ((= (modulo n 15) 0) :fizzbuzz)
        ((= (modulo n 3) 0)  :fizz)
        ((= (modulo n 5) 0)  :buzz)
        (#t                  :num))))

(defmethod fizzbuzz :fizzbuzz (fn (n) "FizzBuzz"))
(defmethod fizzbuzz :fizz     (fn (n) "Fizz"))
(defmethod fizzbuzz :buzz     (fn (n) "Buzz"))
(defmethod fizzbuzz :num      (fn (n) n))

(fizzbuzz 15)  ; => "FizzBuzz"
(fizzbuzz 7)   ; => 7
```
