---
outline: [2, 3]
---

# Components

Components are functions that return [SIP markup](./index#sip-markup-sema-interface-primitives). When mounted to a DOM element, they automatically re-render whenever the reactive state they depend on changes.

## API Reference

### `(defcomponent name [params] body...)`

Defines a component. This is a macro that expands to a regular `define` -- components are just functions that return SIP vectors.

```sema
(defcomponent greeting ()
  [:h1 "Hello, world!"])

(defcomponent greeting-with-name (name)
  [:h1 "Hello, " name "!"])
```

`defcomponent` is syntactic sugar. These are equivalent:

```sema
(defcomponent counter-view ()
  [:p @count])

;; expands to:
(define counter-view
  (fn () [:p @count]))
```

A component with a parameter list expands to a function that accepts an
optional props map and defaults it to `{}`, so `(greeting)` and
`(greeting {:name "Ada"})` are both valid calls.

### `(mount! selector component-fn)`

Mounts a component to a DOM element identified by a CSS selector. The component renders immediately and re-renders automatically when its signal dependencies change.

```sema
(defcomponent app ()
  [:div [:p "Hello"]])

(mount! "#app" "app")
```

The second argument names the component function; a string or a bare symbol
(`(mount! "#app" app)`) both work, and the macro converts a symbol to its name.
The name is how the runtime calls back into the Sema interpreter, so pass the
name of a top-level definition, not a lambda value. The lower-level
`component/mount!` accepts only the string form.

If a component is already mounted at the given selector, it is unmounted first.


`mount!` is an alias for `(component/mount! selector name props)`. The optional
third argument is a map of props passed to the component function.

### `(local name initial)` -- Component-Scoped State

Creates reactive state scoped to the current component. Unlike hooks in React, local state is keyed by **name**, not call order. This means:

- You can call `local` inside conditionals
- You can call `local` in any order
- The name must be unique within the component

```sema
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

```sema
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

### `(effect deps fn)` -- Run Work After Render

Runs `fn` after the component's DOM has been patched, and again whenever `deps`
change. `fn` may return a cleanup -- a function value or a global's name -- which
runs before the next re-run and once when the component is destroyed.

```sema
(defcomponent clock ()
  (let ((now (local "now" 0)))
    (effect (list)
      (fn ()
        (let ((id (js/set-interval (fn () (put! now (+ @now 1))) 1000)))
          (fn () (js/clear-interval id)))))

    [:time (number->string @now)]))
```

`deps` controls when the body re-runs:

| `deps`        | Behaviour                                        |
| ------------- | ------------------------------------------------ |
| `(list)`      | Runs once, cleans up at unmount                  |
| `(list a b)`  | Re-runs whenever `a` or `b` changes              |
| `nil`         | Re-runs after every render                       |

Dependencies are compared **structurally**, so a map or list dep counts as
changed only when its contents differ -- not merely because the render built a
new one.

Anything an effect body creates -- intervals, watches, streams, state -- is owned
by the component and disposed with it. That is a **teardown** guarantee, not a
re-run guarantee: an effect whose deps change runs its body again, and the
second run's interval or watch is added to the first one's. So a body that
creates something and can re-run must return a cleanup that undoes it:

```sema
;; Without the cleanup, changing @topic leaves the old subscription live and
;; the callback fires once per past run.
(effect (list @topic)
  (fn ()
    (let ((id (js/set-interval poll 1000)))
      (fn () (js/clear-interval id)))))
```

An effect with `(list)` deps runs once, so for that shape the cleanup is only
about teardown.

## Composed children and `:key`

A child rendered with `component/render` gets its own scope: its `effect`,
`on-unmount`, `local`, and `resource` registrations belong to the child
instance, not to the component that mounted it.

Repeated children need a `:key` **in their props** to be told apart:

```sema
(defcomponent row (props)
  (def draft (local "draft" ""))
  (effect (list (:id props)) (fn () (subscribe (:id props))))
  [:li {:key (:id props)} @draft])

(defcomponent page ()
  [:ul (map (fn (r) (component/render row (assoc r :key (:id r)))) @rows)])
```

Note the key appears twice, and both matter: `{:key ...}` on the `[:li]` gives
the **DOM** stable identity, and `:key` in the props map gives the child's
**state** the same identity. The SIP attribute is invisible to the component
system, so a keyed list without the prop falls back to sibling ordinals — and
removing a middle row then shifts every row after it onto its neighbour's
state, exactly as an unkeyed list does in any framework.

A child that appears once, or a conditional branch between two different
children, needs no key.

::: warning Register effects unconditionally
Effects are matched across renders by **call order**, not by name (unlike
`local`). Put `effect` and `on-unmount` at the top level of the component body,
never inside an `if` or a `map` callback -- a render that registers a different
number of effects re-keys the ones that follow.
:::



Effects flush at the end of each render, so a first-render effect body runs
*before* the `on-mount` callback.

### `(on-unmount fn)` -- Teardown Hook

Runs `fn` once when the component is destroyed, whether by
`(component/unmount! selector)`, by mounting something else over the same
selector, or by disposing the whole runtime.

```sema
(defcomponent session-view ()
  (on-unmount (fn () (console/log "session view went away")))
  [:p "active"])
```

Hooks run in reverse registration order, so the last one registered tears down
first. Like `effect`, `on-unmount` is matched by call order -- register it
unconditionally.

A hook runs at teardown and nowhere else. If a later render changes the slot
sequence -- it registers an extra effect above the hook, or stops registering
the hook at all -- the runtime reports it through the `onerror` handler as
`lifecycle:<component>#<slot>` rather than
running the hook while the component is still on screen. A hook a render
stopped registering never runs, which is why the rule is "register
unconditionally" rather than "prefer to register unconditionally".

Errors thrown by an effect body, an effect cleanup, or an `on-unmount` hook are
reported through the runtime's `onerror` handler and never abort teardown; the
remaining cleanups still run.

### `(component/unmount! selector)` -- Unmount

Removes a mounted component, runs its cleanup function (if any), clears the mount target, and stops reactive tracking.

```sema
(component/unmount! "#app")
```

### `(component/force-render! selector)` -- Force Re-render

Triggers a re-render even if no signal dependencies changed. Rarely needed, but useful for debugging.

```sema
(component/force-render! "#app")
```

Call it from an event handler or from ordinary code -- never from a render body
or an effect body of the component it names. That component is already
rendering, so the request is refused and reported as
`force-render:<component>`; a component re-renders on its own when the state it
reads changes.

## Event Handling

Events are handled through **delegated event listeners**. In SIP markup, `on-*` attributes specify the name of a Sema function to call:

```sema
(define (handle-click ev)
  (console/log "Clicked!"))

(defcomponent app ()
  [:button {:on-click "handle-click"} "Click me"])
```

The event handler receives a handle to the DOM event. You can extract data from it:

```sema
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

Delegation works by listening on the mount root, so the events it can route are
the ones that bubble to it. These are the whole set:

| Category | Events |
| --- | --- |
| Mouse | `on-click`, `on-dblclick`, `on-auxclick`, `on-contextmenu`, `on-mousedown`, `on-mouseup`, `on-mousemove`, `on-mouseover`, `on-mouseout`, `on-wheel`, `on-mouseenter`, `on-mouseleave` |
| Pointer | `on-pointerdown`, `on-pointerup`, `on-pointermove`, `on-pointerover`, `on-pointerout`, `on-pointercancel` |
| Touch | `on-touchstart`, `on-touchend`, `on-touchmove`, `on-touchcancel` |
| Keyboard | `on-keydown`, `on-keyup`, `on-keypress` |
| Form | `on-input`, `on-change`, `on-submit`, `on-reset`, `on-select` |
| Focus | `on-focusin`, `on-focusout` |
| Clipboard | `on-copy`, `on-cut`, `on-paste` |
| Drag & drop | `on-drag`, `on-dragstart`, `on-dragend`, `on-dragenter`, `on-dragleave`, `on-dragover`, `on-drop` |
| Animation | `on-animationstart`, `on-animationend`, `on-transitionend` |

Event handler values are **always strings** -- the name of a defined Sema function.

Anything else is refused rather than rendered dead. A misspelled name
(`{:on-sumbit.prevent "save"}`) reports through `onerror` under
`sip-render:on-handler` with the correction named, and installs no handler --
because a `.prevent` that silently never runs lets the form navigate away, which
is the entire symptom a typo would otherwise have.

`focus` and `blur` do not bubble; use `on-focusin` / `on-focusout`, which do.
For a custom element's event or another non-bubbling one (`scroll`), attach the
listener yourself from an `on-mount` callback:

```sema
(defcomponent picker ()
  (on-mount (fn ()
    (let ((el (dom/query "#picker")))
      (dom/on! el "picker-change" "handle-pick")
      (fn () (dom/off! el "picker-change" "handle-pick")))))
  [:my-picker {:id "picker"}])
```

`mouseenter` and `mouseleave` are synthesized from `mouseover`/`mouseout`. Every
element the pointer actually entered runs its handler, outermost first, and
every element it left runs its handler innermost first -- so a hover counter
kept by a nested pair of handlers stays balanced.

## Re-rendering and Diffing

Components re-render via `@preact/signals-core`'s `effect()`. When a signal dependency changes:

1. The component function is called again, producing new SIP markup
2. SIP markup is rendered to DOM nodes
3. `morphdom` patches the existing DOM to match, minimizing mutations

### Focus Preservation

morphdom is configured to preserve focus state. If the user is typing in an input field and a re-render occurs, the input retains focus and cursor position.

Everything else about the element is still patched: attributes the new render declares are applied, attributes it **no longer** declares are removed (including a dropped `on-*`, whose handler stops firing with it), and a focused `<select>` still gains and loses `<option>`s -- a dependent dropdown must update for the user who has it focused. What is left alone is the live state the user owns: the `value` and `checked` properties, the caret, and which options are selected.

### What Triggers a Re-render

Only signals read via `@` during the component's render are tracked. Event handlers, `watch` callbacks, and `on-mount` code do not create subscriptions.

```sema
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

```sema
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

```sema
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
