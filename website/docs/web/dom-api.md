# DOM API

The `dom/*` namespace provides a thin wrapper over the browser DOM API. All functions operate on **numeric handles** -- opaque IDs that reference DOM elements, text nodes, or events across the WASM boundary.

## Query

### `(dom/query selector)` -> handle | nil

Find the first element matching a CSS selector.

```scheme
(def el (dom/query ".my-class"))
(def nav (dom/query "nav > ul"))
```

### `(dom/query-all selector)` -> list of handles

Find all elements matching a CSS selector.

```scheme
(def items (dom/query-all "li.todo"))
```

### `(dom/get-id id)` -> handle | nil

Find an element by its `id` attribute.

```scheme
(def app (dom/get-id "app"))
```

## Create

### `(dom/create-element tag)` -> handle

Create a new DOM element.

```scheme
(def div (dom/create-element "div"))
```

### `(dom/create-text content)` -> handle

Create a text node.

```scheme
(def txt (dom/create-text "Hello, world!"))
```

## Tree Manipulation

### `(dom/append-child! parent-handle child-handle)` -> child-handle

Append a child node to a parent element. Returns the child handle.

```scheme
(def container (dom/get-id "app"))
(def p (dom/create-element "p"))
(dom/set-text! p "New paragraph")
(dom/append-child! container p)
```

### `(dom/remove-child! parent-handle child-handle)` -> child-handle

Remove a child node from its parent.

```scheme
(dom/remove-child! container p)
```

### `(dom/remove! handle)` -> nil

Remove an element from the DOM entirely.

```scheme
(dom/remove! (dom/query ".obsolete"))
```

## Attributes

### `(dom/set-attribute! handle attr value)` -> nil

```scheme
(dom/set-attribute! el "data-count" "5")
```

### `(dom/get-attribute handle attr)` -> string | nil

```scheme
(dom/get-attribute el "href")
```

### `(dom/remove-attribute! handle attr)` -> nil

```scheme
(dom/remove-attribute! el "disabled")
```

## CSS Classes

### `(dom/add-class! handle class ...)` -> nil

Add one or more CSS classes.

```scheme
(dom/add-class! el "active" "highlighted")
```

### `(dom/remove-class! handle class ...)` -> nil

Remove one or more CSS classes.

```scheme
(dom/remove-class! el "active")
```

### `(dom/toggle-class! handle class)` -> boolean

Toggle a CSS class. Returns `true` if the class is now present, `false` otherwise.

```scheme
(dom/toggle-class! el "expanded")
```

### `(dom/has-class? handle class)` -> boolean

Check whether an element has a CSS class.

```scheme
(if (dom/has-class? el "active")
  (println "Element is active"))
```

## Styles

### `(dom/set-style! handle property value)` -> nil

Set a CSS style property. Use kebab-case property names.

```scheme
(dom/set-style! el "background-color" "#f0f0f0")
(dom/set-style! el "font-size" "16px")
```

### `(dom/get-style handle property)` -> string

Get a CSS style property value.

```scheme
(dom/get-style el "color")
```

## Content

### `(dom/set-text! handle text)` -> nil

Set the `textContent` of an element.

```scheme
(dom/set-text! el "Updated content")
```

### `(dom/get-text handle)` -> string

Get the `textContent` of an element.

### `(dom/set-html! handle html)` -> nil

Set the `innerHTML` of an element. Use with caution -- no sanitization is performed.

```scheme
(dom/set-html! el "<strong>Bold</strong>")
```

### `(dom/get-html handle)` -> string

Get the `innerHTML` of an element.

## Form Values

### `(dom/set-value! handle value)` -> nil

Set the `value` property of an input element.

```scheme
(dom/set-value! input "default text")
```

### `(dom/get-value handle)` -> string

Get the `value` property of an input element.

```scheme
(def text (dom/get-value input))
```

### `(dom/event-value event-handle)` -> string | nil

Read `event.target.value` from an event handle. Useful in input event handlers:

```scheme
(define (on-input ev)
  (def val (dom/event-value ev))
  (println "Input:" val))
```

## Events

### `(dom/on! handle event callback)` -> nil

Add an event listener. The callback may be either:

- a function value
- a callback name string for an existing top-level function

The callback receives a numeric event handle as its argument.

```scheme
(define (handle-click ev)
  (dom/prevent-default! ev)
  (println "Clicked!"))

(dom/on! btn "click" handle-click)
;; or:
(dom/on! btn "click" "handle-click")
```

The event handle is automatically released after the callback returns.

### `(dom/off! handle event callback)` -> nil

Remove a previously registered event listener.

```scheme
(dom/off! btn "click" handle-click)
;; or:
(dom/off! btn "click" "handle-click")
```

### `(dom/prevent-default! event-handle)` -> nil

Call `preventDefault()` on an event.

```scheme
(define (on-submit ev)
  (dom/prevent-default! ev)
  ;; handle form submission
  )
```

## SIP Rendering

### `(dom/render sip-data)` -> handle

Render a SIP vector into a DOM element and return its handle. See [SIP Markup](./sip-markup.md) for the format.

```scheme
(def card (dom/render [:div {:class "card"} "Hello"]))
```

### `(dom/render-into! selector sip-data)` -> nil

Render SIP data into the element matching `selector`, replacing existing content.

```scheme
(dom/render-into! "#app"
  [:div [:h1 "Hello, world!"]])
```

## Notes

- All handles are numeric IDs managed by an internal handle map. They reference DOM elements, text nodes, or events.
- `dom/on!` accepts either a function value or a callback-name string. `dom/off!` must be given the same callback identity that was used when registering the listener.
- When using `dom/on!` on elements inside a component rendered with morphdom, be aware that morphdom may replace DOM nodes, orphaning your listeners. Prefer SIP `on-*` attributes for components that re-render.
