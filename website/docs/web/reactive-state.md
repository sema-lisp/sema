---
outline: [2, 3]
---

# Reactive State

Sema Web's reactivity is built on `@preact/signals-core`. State is held in **signals** -- when a signal's value changes, anything that read it (components, computed values, watchers) automatically updates.

## API Reference

### `(state val)` -- Create Reactive State

Creates a new signal with the given initial value. Returns a signal reference (an opaque numeric ID).

```scheme
(def count (state 0))
(def name (state "Sema"))
(def items (state '(1 2 3)))
(def user (state {:name "Alice" :age 30}))
```

Signals can hold any Sema value: numbers, strings, lists, maps, booleans, `nil`.

### `@x` -- Read Value

The `@` reader macro dereferences a signal, returning its current value. Inside a component render function or `computed` expression, reading with `@` automatically subscribes to changes.

```scheme
(def count (state 0))

@count          ;; => 0
(+ @count 1)    ;; => 1
```

`@x` expands to `(deref x)` at read time. You can use `deref` directly if you prefer:

```scheme
(deref count)   ;; same as @count
```

### `(put! x val)` -- Set Value

Replaces the signal's value. Triggers re-renders for any subscribed components or computed values.

```scheme
(def count (state 0))
(put! count 42)
@count  ;; => 42

(def items (state '()))
(put! items '(1 2 3))
```

### `(update! x fn . args)` -- Apply Function

Reads the current value, applies the function with the current value as the first argument (plus any additional args), and writes the result back.

```scheme
(def count (state 0))
(update! count (fn (n) (+ n 1)))   ;; count is now 1
(update! count (fn (n) (+ n 10)))  ;; count is now 11

;; With extra arguments
(def items (state '(1 2)))
(update! items (fn (lst x) (append lst (list x))) 3)
;; items is now (1 2 3)
```

`update!` is equivalent to `(put! x (apply fn (cons @x args)))`.

### `(computed expr)` -- Derived State

Creates a read-only signal whose value is computed from an expression. Dependencies are tracked automatically -- when any signal read inside the expression changes, the computed value updates.

```scheme
(def count (state 0))
(def doubled (computed (* @count 2)))
(def message (computed (string-append "Count is " (number->string @count))))

@doubled   ;; => 0
(put! count 5)
@doubled   ;; => 10
@message   ;; => "Count is 5"
```

`computed` is a macro. It wraps the expression in a thunk that the runtime calls inside `@preact/signals-core`'s `computed()`, so dependency tracking works transparently.

::: warning
`computed` expressions should be pure -- no side effects, no `put!` calls. Use `watch` for side effects.
:::

### `(batch body...)` -- Coalesce Updates

Groups multiple state mutations into a single update pass. Without `batch`, each `put!` triggers an immediate re-render. With `batch`, re-renders are deferred until the batch completes.

```scheme
(def first-name (state ""))
(def last-name (state ""))

;; Without batch: two re-renders
(put! first-name "Ada")
(put! last-name "Lovelace")

;; With batch: one re-render
(batch
  (put! first-name "Ada")
  (put! last-name "Lovelace"))
```

Use `batch` when updating multiple related signals to avoid intermediate renders with inconsistent state.

### `(watch x fn)` -- Side Effects

Observes a signal and calls a function whenever the value changes. The function receives the old and new values as arguments.

`watch` returns a numeric watch handle. Call `unwatch!` with that handle to stop observing.

```scheme
(def count (state 0))

(define (log-change old new)
  (console/log "count changed from" old "to" new))

(def stop-id (watch count log-change))

(put! count 1)   ;; logs: count changed from 0 to 1
(put! count 2)   ;; logs: count changed from 1 to 2

(unwatch! stop-id)
```

Common uses for `watch`:

- Logging or analytics
- Syncing to localStorage
- Triggering network requests
- Updating document title

::: tip
Do not use `watch` to update other signals that drive rendering -- use `computed` instead. Watches are for effects outside the reactive graph (network, storage, logging).
:::

### `(unwatch! watch-id)` -- Stop Watching

Disposes a watch created by `watch`.

```scheme
(def watch-id (watch count log-change))
(unwatch! watch-id)
```

## Complete Example: Todo List

```scheme
;; --- State ---
(def todos (state '()))
(def next-id (state 1))
(def filter-mode (state "all"))  ;; "all", "active", "done"

;; Derived state
(def visible-todos
  (computed
    (let ((mode @filter-mode)
          (all @todos))
      (cond
        ((equal? mode "active") (filter (fn (t) (not (get t :done))) all))
        ((equal? mode "done")   (filter (fn (t) (get t :done)) all))
        (else all)))))

(def active-count
  (computed (length (filter (fn (t) (not (get t :done))) @todos))))

;; --- Actions ---
(define (add-todo text)
  (batch
    (let ((id @next-id))
      (update! todos (fn (lst)
        (append lst (list {:id id :text text :done false}))))
      (update! next-id (fn (n) (+ n 1))))))

(define (toggle-todo id)
  (update! todos (fn (lst)
    (map (fn (t)
      (if (equal? (get t :id) id)
          (assoc t :done (not (get t :done)))
          t))
      lst))))

(define (remove-todo id)
  (update! todos (fn (lst)
    (filter (fn (t) (not (equal? (get t :id) id))) lst))))

;; Persist to localStorage
(watch todos (fn (old new)
  (store/set "todos" (json/encode new))))
```

## Auto-tracking: How It Works

When a component renders or a `computed` expression evaluates, Sema Web runs the code inside a signals-core `effect()` or `computed()` context. Every `@` (deref) call inside that context registers a dependency on the underlying signal.

```scheme
(def a (state 1))
(def b (state 2))

;; This computed depends on both `a` and `b`
(def sum (computed (+ @a @b)))

;; Updating either triggers recomputation
(put! a 10)   ;; sum becomes 12
(put! b 20)   ;; sum becomes 30
```

Dependencies are tracked dynamically, not statically. If a branch is not taken, those signals are not subscribed:

```scheme
(def show-detail (state false))
(def detail (state "..."))

(defcomponent view ()
  [:div
    (if @show-detail
        [:p @detail]       ;; only subscribes to `detail` when show-detail is true
        [:p "Summary"])])
```

## Comparison with Other Frameworks

| Concept | Sema Web | React | Vue 3 | Solid |
| --- | --- | --- | --- | --- |
| Create state | `(state 0)` | `useState(0)` | `ref(0)` | `createSignal(0)` |
| Read | `@count` | `count` | `count.value` | `count()` |
| Write | `(put! count 1)` | `setCount(1)` | `count.value = 1` | `setCount(1)` |
| Update | `(update! count inc)` | `setCount(c => c+1)` | `count.value++` | `setCount(c => c+1)` |
| Derived | `(computed expr)` | `useMemo(fn, deps)` | `computed(fn)` | `createMemo(fn)` |
| Batch | `(batch ...)` | Automatic in events | `nextTick` | `batch(fn)` |
| Side effect | `(watch x fn)` | `useEffect` | `watch(x, fn)` | `createEffect` |
| Local state | `(local "n" 0)` | `useState(0)` | `ref(0)` in setup | `createSignal(0)` |

Key differences from React:

- **No dependency arrays.** Auto-tracking means you never forget a dependency.
- **No stale closures.** `@count` always reads the current value.
- **No hooks rules.** `local` is keyed by name, not call order. Call it conditionally if you want.
- **Fine-grained updates.** Only the specific DOM nodes that depend on a signal are patched, not the entire component subtree.

## Common Patterns

### Derived Filtered List

```scheme
(def items (state '(1 2 3 4 5 6 7 8 9 10)))
(def min-val (state 5))

(def filtered (computed (filter (fn (x) (>= x @min-val)) @items)))

@filtered  ;; => (5 6 7 8 9 10)
(put! min-val 8)
@filtered  ;; => (8 9 10)
```

### Form State

```scheme
(def form-data (state {:name "" :email ""}))

(define (set-field field value)
  (update! form-data (fn (m) (assoc m field value))))

(define (handle-name-input ev)
  (set-field :name (dom/event-value ev)))

(define (handle-email-input ev)
  (set-field :email (dom/event-value ev)))
```

### Undo/Redo

```scheme
(def history (state '()))
(def future (state '()))
(def current (state nil))

(define (push-state val)
  (batch
    (update! history (fn (h) (cons @current h)))
    (put! future '())
    (put! current val)))

(define (undo)
  (when (not (null? @history))
    (batch
      (update! future (fn (f) (cons @current f)))
      (put! current (car @history))
      (update! history cdr))))
```

## Gotchas

**Deref outside reactive context.** `@count` works anywhere, but outside a component or `computed`, it just reads the value without subscribing. This is fine for event handlers and one-off reads.

**Mutating nested structures.** Signals track identity, not deep equality. To update a field in a map, you must `put!` or `update!` with a new map -- mutating the map in place will not trigger updates.

**Computed must be synchronous.** The expression inside `computed` runs synchronously. For async derived data, use `watch` to observe a signal and update another signal in the callback.
