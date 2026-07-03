---
outline: [2, 3]
---

# Components

Components are functions that return [SIP markup](./index#sip-markup-sema-interface-primitives). When mounted to a DOM element, they automatically re-render whenever the reactive state they depend on changes.

## API Reference

### `(defcomponent name [params] body...)`

Defines a component. This is a macro that expands to a regular `define` -- components are just functions that return SIP vectors.

```scheme
(defcomponent greeting ()
  [:h1 "Hello, world!"])

(defcomponent greeting-with-name (name)
  [:h1 "Hello, " name "!"])
```

`defcomponent` is syntactic sugar. These are equivalent:

```scheme
(defcomponent counter-view ()
  [:p @count])

;; expands to:
(define counter-view
  (fn () [:p @count]))
```

### `(mount! selector component-fn)`

Mounts a component to a DOM element identified by a CSS selector. The component renders immediately and re-renders automatically when its signal dependencies change.

```scheme
(defcomponent app ()
  [:div [:p "Hello"]])

(mount! "#app" "app")
```

The second argument is the **name** of the component function as a string, not the function itself. This is how the runtime calls back into the Sema interpreter.

If a component is already mounted at the given selector, it is unmounted first.

::: warning
`mount!` takes a **string** name: `(mount! "#app" "app")`, not `(mount! "#app" app)`.
:::

### `(local name initial)` -- Component-Scoped State

Creates reactive state scoped to the current component. Unlike hooks in React, local state is keyed by **name**, not call order. This means:

- You can call `local` inside conditionals
- You can call `local` in any order
- The name must be unique within the component

```scheme
(defcomponent counter ()
  (let ((count (local "count" 0)))
    [:div
      [:p "Count: " @count]]))
```

On the first render, `(local "count" 0)` creates a new signal with value `0`. On subsequent re-renders, it returns the same signal -- the initial value is ignored.

`local` returns a signal reference, so you read it with `@` and write it with `put!` or `update!`, just like top-level state.

### `(on-mount fn)` -- Lifecycle Hook

Registers a function to call once after the component's first render. The callback can return either:

- a cleanup function value
- a cleanup function name string

That cleanup runs when the component is unmounted.

```scheme
(defcomponent timer ()
  (let ((elapsed (local "elapsed" 0))
        (interval-id (local "interval-id" nil)))

    (define (tick) (update! elapsed (fn (n) (+ n 1))))

    (define (cleanup)
      (when @interval-id
        (js/clear-interval @interval-id)))

    (on-mount (fn ()
      (put! interval-id (js/set-interval tick 1000))
      cleanup))   ;; returning the function value is preferred

    [:p "Elapsed: " @elapsed "s"]))
```

Key points:

- `on-mount` runs after the DOM is painted, not during render
- The cleanup function runs when `component/unmount!` is called on the selector
- Call `on-mount` only once per component -- the last call wins

### `(component/unmount! selector)` -- Unmount

Removes a mounted component, runs its cleanup function (if any), clears the mount target, and stops reactive tracking.

```scheme
(component/unmount! "#app")
```

### `(component/force-render! selector)` -- Force Re-render

Triggers a re-render even if no signal dependencies changed. Rarely needed, but useful for debugging.

```scheme
(component/force-render! "#app")
```

## Event Handling

Events are handled through **delegated event listeners**. In SIP markup, `on-*` attributes specify the name of a Sema function to call:

```scheme
(define (handle-click ev)
  (console/log "Clicked!"))

(defcomponent app ()
  [:button {:on-click "handle-click"} "Click me"])
```

The event handler receives a handle to the DOM event. You can extract data from it:

```scheme
(define (handle-input ev)
  (let ((value (dom/event-value ev)))
    (put! search-text value)))

(defcomponent search ()
  [:input {:type "text"
           :value @search-text
           :on-input "handle-input"
           :placeholder "Search..."}])
```

### Supported Events

All standard DOM events are supported via delegation:

| Category | Events |
| --- | --- |
| Mouse | `on-click`, `on-dblclick`, `on-contextmenu`, `on-mouseenter`, `on-mouseleave` |
| Pointer | `on-pointerdown`, `on-pointerup`, `on-pointermove` |
| Keyboard | `on-keydown`, `on-keyup`, `on-keypress` |
| Form | `on-input`, `on-change`, `on-submit` |
| Focus | `on-focusin`, `on-focusout` |

Event handler values are **always strings** -- the name of a defined Sema function.

## Re-rendering and Diffing

Components re-render via `@preact/signals-core`'s `effect()`. When a signal dependency changes:

1. The component function is called again, producing new SIP markup
2. SIP markup is rendered to DOM nodes
3. `morphdom` patches the existing DOM to match, minimizing mutations

### Focus Preservation

morphdom is configured to preserve focus state. If the user is typing in an input field and a re-render occurs, the input retains focus and cursor position. Attributes (like `class`) are still updated, but the `value` property is left alone for the active element.

### What Triggers a Re-render

Only signals read via `@` during the component's render are tracked. Event handlers, `watch` callbacks, and `on-mount` code do not create subscriptions.

```scheme
(def a (state 1))
(def b (state 2))

(defcomponent example ()
  ;; This component subscribes to `a` only
  [:p "Value: " @a])

;; Changing `a` re-renders the component
(put! a 10)

;; Changing `b` does NOT re-render -- it was never read during render
(put! b 20)
```

## Full Example: Timer with Cleanup

```scheme
;; A timer that counts seconds and cleans up on unmount

(def elapsed (state 0))
(def timer-id (state nil))

(define (tick)
  (update! elapsed (fn (n) (+ n 1))))

(define (start-timer)
  (put! timer-id (js/set-interval "tick" 1000)))

(define (stop-timer)
  (when @timer-id
    (js/clear-interval @timer-id)
    (put! timer-id nil)))

(define (reset-timer ev)
  (batch
    (stop-timer)
    (put! elapsed 0)
    (start-timer)))

(define (cleanup-timer)
  (stop-timer))

(defcomponent timer-view ()
  (on-mount (fn ()
    (start-timer)
    "cleanup-timer"))

  (let ((mins (quotient @elapsed 60))
        (secs (remainder @elapsed 60)))
    [:div {:class "timer"}
      [:p (string-append
            (number->string mins) "m "
            (number->string secs) "s")]
      [:button {:on-click "reset-timer"} "Reset"]]))

(mount! "#app" "timer-view")
```

## Full Example: Todo App

```scheme
;; --- State ---
(def todos (state '()))
(def next-id (state 1))

;; --- Actions ---
(define (add-todo ev)
  (let ((input (dom/query "#todo-input")))
    (let ((text (dom/get-attribute input "value")))
      (when (not (equal? text ""))
        (batch
          (update! todos (fn (lst)
            (append lst (list {:id @next-id :text text :done false}))))
          (update! next-id (fn (n) (+ n 1))))
        (dom/set-attribute! input "value" "")))))

(define (toggle ev)
  ;; Get the todo ID from the event target's data attribute
  (let ((id (string->number (dom/get-attribute (dom/event-target ev) "data-id"))))
    (update! todos (fn (lst)
      (map (fn (t)
        (if (equal? (get t :id) id)
            (assoc t :done (not (get t :done)))
            t))
        lst)))))

(define (remove ev)
  (let ((id (string->number (dom/get-attribute (dom/event-target ev) "data-id"))))
    (update! todos (fn (lst)
      (filter (fn (t) (not (equal? (get t :id) id))) lst)))))

;; --- Components ---
(defcomponent todo-item (todo)
  (let ((done? (get todo :done))
        (id (number->string (get todo :id))))
    [:li {:class (if done? "done" "")}
      [:span {:on-click "toggle" :data-id id}
        (get todo :text)]
      [:button {:on-click "remove" :data-id id} "x"]]))

(defcomponent app ()
  [:div {:class "todo-app"}
    [:h1 "Todos"]
    [:div {:class "input-row"}
      [:input {:id "todo-input" :type "text" :placeholder "What needs doing?"}]
      [:button {:on-click "add-todo"} "Add"]]
    [:ul
      (map (fn (t) (todo-item t)) @todos)]])

(mount! "#app" "app")
```

## Gotchas

**SIP event handlers still use names.** `{:on-click "my-fn"}` passes the string `"my-fn"`. SIP delegated event attributes are still name-based even though lower-level APIs like `dom/on!`, `watch`, and `on-mount` now accept function values.

**`local` needs a string name.** `(local "count" 0)` not `(local count 0)`. The name is used as a stable key across re-renders.

**`on-mount` timing.** The callback runs after the first render is painted to the DOM, not during the render function. Do not read signal values inside `on-mount` to drive rendering -- use the component body for that.

**Avoid `dom/on!` inside components.** Event listeners added with `dom/on!` are lost on re-render because morphdom replaces elements. Use `{:on-click "handler"}` in SIP attributes instead -- these use delegated event handling that survives DOM patches.

**Nested components.** Call component functions directly in the parent's SIP output. They are regular function calls, not mount points. Only the top-level `mount!` creates a reactive boundary.

## Related

- [Reactive State](./reactive-state) -- `state`, `put!`, `update!`, `computed`, `batch`, `watch`
- [Getting Started](./getting-started) -- setting up your first page
