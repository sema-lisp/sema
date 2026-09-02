# DOM API

The `dom/*` namespace provides a thin wrapper over the browser DOM API. All functions operate on **numeric handles** -- opaque IDs that reference DOM elements, text nodes, or events across the WASM boundary.

## Query

### `(dom/query selector)` -> handle | nil

Find the first element matching a CSS selector.

```sema
(def el (dom/query ".my-class"))
(def nav (dom/query "nav > ul"))
```

### `(dom/query-all selector)` -> list of handles

Find all elements matching a CSS selector.

```sema
(def items (dom/query-all "li.todo"))
```

### `(dom/get-id id)` -> handle | nil

Find an element by its `id` attribute.

```sema
(def app (dom/get-id "app"))
```

## Create

### `(dom/create-element tag)` -> handle

Create a new DOM element.

```sema
(def div (dom/create-element "div"))
```

### `(dom/create-text content)` -> handle

Create a text node.

```sema
(def txt (dom/create-text "Hello, world!"))
```

## Tree Manipulation

### `(dom/append-child! parent-handle child-handle)` -> child-handle

Append a child node to a parent element. Returns the child handle.

```sema
(def container (dom/get-id "app"))
(def p (dom/create-element "p"))
(dom/set-text! p "New paragraph")
(dom/append-child! container p)
```

### `(dom/remove-child! parent-handle child-handle)` -> child-handle

Remove a child node from its parent.

```sema
(dom/remove-child! container p)
```

### `(dom/remove! handle)` -> nil

Remove an element from the DOM entirely.

```sema
(dom/remove! (dom/query ".obsolete"))
```

## Attributes

### `(dom/set-attribute! handle attr value)` -> nil

```sema
(dom/set-attribute! el "data-count" "5")
```

### `(dom/get-attribute handle attr)` -> string | nil

```sema
(dom/get-attribute el "href")
```

### `(dom/remove-attribute! handle attr)` -> nil

```sema
(dom/remove-attribute! el "disabled")
```

## CSS Classes

### `(dom/add-class! handle class ...)` -> nil

Add one or more CSS classes.

```sema
(dom/add-class! el "active" "highlighted")
```

### `(dom/remove-class! handle class ...)` -> nil

Remove one or more CSS classes.

```sema
(dom/remove-class! el "active")
```

### `(dom/toggle-class! handle class)` -> boolean

Toggle a CSS class. Returns `true` if the class is now present, `false` otherwise.

```sema
(dom/toggle-class! el "expanded")
```

### `(dom/has-class? handle class)` -> boolean

Check whether an element has a CSS class.

```sema
(if (dom/has-class? el "active")
  (println "Element is active"))
```

## Styles

### `(dom/set-style! handle property value)` -> nil

Set a CSS style property. Use kebab-case property names.

```sema
(dom/set-style! el "background-color" "#f0f0f0")
(dom/set-style! el "font-size" "16px")
```

### `(dom/get-style handle property)` -> string

Get an **inline** style property value (the element's `style` attribute). A
value set by a stylesheet or by `css` is not visible here and reads as `""`.

```sema
(dom/get-style el "color")
```

## Content

### `(dom/set-text! handle text)` -> nil

Set the `textContent` of an element.

```sema
(dom/set-text! el "Updated content")
```

### `(dom/get-text handle)` -> string

Get the `textContent` of an element.

### `(dom/set-html! handle html)` -> nil

Set the `innerHTML` of an element. Use with caution -- no sanitization is performed.

```sema
(dom/set-html! el "<strong>Bold</strong>")
```

### `(dom/get-html handle)` -> string

Get the `innerHTML` of an element.

## Form Values

### `(dom/set-value! handle value)` -> nil

Set the `value` property of an input element.

```sema
(dom/set-value! input "default text")
```

### `(dom/get-value handle)` -> string

Get the `value` property of an input element.

```sema
(def text (dom/get-value input))
```

### `(dom/event-value event-handle)` -> string | nil

Read `event.target.value` from an event handle. Useful in input event handlers:

```sema
(define (on-input ev)
  (def val (dom/event-value ev))
  (println "Input:" val))
```

### `(dom/event-checked event-handle)` -> boolean | nil

Read `event.target.checked`. `nil` when the target has no checked state.

```sema
(define (on-toggle ev)
  (put! enabled (dom/event-checked ev)))
```

### `(dom/checked? handle)` -> boolean

Read the `checked` state of an element handle. `#f` for anything that has none.

### `(dom/selected-values handle)` -> list

Values of every selected `<option>` in a `<select>`. A single select yields a
one-element list, a multi-select with nothing selected yields `()`, and
anything that is not a select yields `()`.

```sema
(def sizes (dom/selected-values select-el))
```

### `(dom/event-selected-values event-handle)` -> list | nil

The same, read from the event's target. `nil` for a non-element target.

## Forms

### `(dom/event-form-data event-handle)` -> map | nil

Every submittable field of the form the event came from, keyed by field name.
`nil` when the event target owns no form.

```sema
(define (save ev)
  (def fields (dom/event-form-data ev))
  (println (:title fields)))

[:form {:on-submit.prevent "save"}
  [:input {:name "title"}]
  [:input {:name "tag"}]
  [:input {:name "tag"}]
  [:button {:type "submit"} "Save"]]
```

Value shapes:

- a field name that appears **once** is a plain string;
- a field name that **repeats** is a list, in document order -- the `:tag`
  inputs above read as `("a" "b")`;
- a file input is a map, `{:name "a.txt" :size 3 :type "text/plain"}`;
- unchecked checkboxes, disabled controls, and unnamed controls are absent,
  exactly as in a real form submission;
- the submitting button's own `name`/`value` is included when there is one.

Field names become keywords, so `(:title fields)` works. A name that is not a
valid keyword needs `(get fields (string->keyword "user[name]"))`.

### `(dom/form-data handle)` -> map | nil

The same map, from an element handle. Accepts the `<form>` itself or any
element inside it (or associated with it via `form="<id>"`).

### `(dom/event-form event-handle)` -> handle | nil

The `<form>` the event's target belongs to.

## Events

### `(dom/on! handle event callback)` -> nil

Add an event listener. The callback may be either:

- a function value
- a callback name string for an existing top-level function

The callback receives a numeric event handle as its argument.

```sema
(define (handle-click ev)
  (dom/prevent-default! ev)
  (println "Clicked!"))

(dom/on! btn "click" handle-click)
;; or:
(dom/on! btn "click" "handle-click")
```

The event handle is automatically released after the callback returns.

### `(dom/stop-propagation! event-handle)` -> nil

Stop the event from bubbling to ancestor elements, including delegated SIP
handlers (`{:on-click ...}`) on ancestors.

### `(dom/event-target-closest event-handle selector)` -> handle | nil

Return the closest ancestor of the event target (including the target itself)
that matches `selector`, as an element handle. Useful in a delegated handler on
a list to find which row was clicked.

```sema
(define (handle-row-click ev)
  (when-let (row (dom/event-target-closest ev "tr[data-id]"))
    (println (dom/get-attribute row "data-id"))))
```

### `(dom/focus! handle)` -> nil

Move keyboard focus to the element.

### `(dom/off! handle event callback)` -> nil

Remove a previously registered event listener.

```sema
(dom/off! btn "click" handle-click)
;; or:
(dom/off! btn "click" "handle-click")
```

### `(dom/event-current-target event-handle)` -> handle | nil

The element that declared the handler. For a delegated SIP handler this is the
element carrying the `:on-*` attribute, **not** the mount root that
`event.currentTarget` would report. For a `dom/on!` listener it is the element
the listener was attached to.

```sema
(define (on-row-click ev)
  (def row (dom/event-current-target ev))
  (println (dom/get-attribute row "data-row-id")))
```

### `(dom/prevent-default! event-handle)` -> nil

Call `preventDefault()` on an event.

```sema
(define (on-submit ev)
  (dom/prevent-default! ev)
  ;; handle form submission
  )
```

## SIP Rendering

### `(dom/render sip-data)` -> handle

Render a SIP vector into a DOM element and return its handle. See [SIP Markup](./sip-markup.md) for the format.

```sema
(def card (dom/render [:div {:class "card"} "Hello"]))
```

### `(dom/render-into! selector sip-data)` -> nil

Render SIP data into the element matching `selector`, replacing existing content.

```sema
(dom/render-into! "#app"
  [:div [:h1 "Hello, world!"]])
```

## Notes

- All handles are numeric IDs managed by an internal handle map. They reference DOM elements, text nodes, or events.
- `dom/on!` accepts either a function value or a callback-name string. `dom/off!` must be given the same callback identity that was used when registering the listener.
- When using `dom/on!` on elements inside a component rendered with morphdom, be aware that morphdom may replace DOM nodes, orphaning your listeners. Prefer SIP `on-*` attributes for components that re-render.

## Console

Thin wrappers over the browser console. Every function accepts any values and
returns `nil`.

| Function | Browser call |
| --- | --- |
| `(console/log ...)` | `console.log` |
| `(console/info ...)` | `console.info` |
| `(console/warn ...)` | `console.warn` |
| `(console/error ...)` | `console.error` |
| `(console/debug ...)` | `console.debug` |
| `(console/clear)` | `console.clear` |
| `(console/time label)` / `(console/time-end label)` | `console.time` / `console.timeEnd` |

```sema
(console/time "render")
(render-board)
(console/time-end "render")   ; render: 3.2ms
```
