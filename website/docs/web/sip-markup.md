# SIP Markup

SIP (Sema Interface Primitives) is a declarative format for describing DOM structures using Sema vectors. It follows the hiccup convention: each element is a vector of `[tag, attrs?, ...children]`.

## Format Overview

SIP vectors map directly to HTML elements:

| HTML | SIP |
|------|-----|
| `<div></div>` | `[:div]` |
| `<p>Hello</p>` | `[:p "Hello"]` |
| `<h1 class="title">Hello</h1>` | `[:h1 {:class "title"} "Hello"]` |
| `<a href="/about">About</a>` | `[:a {:href "/about"} "About"]` |
| `<input disabled />` | `[:input {:disabled true}]` |

The general shape is:

```scheme
[:tag-name {:attr "value"} child1 child2 ...]
```

The attribute map is optional. When the second element is not a map, all remaining elements are treated as children.

## Tags

Tags are keywords. The leading colon is stripped during rendering:

```scheme
[:div "content"]       ;; <div>content</div>
[:span "inline"]       ;; <span>inline</span>
[:button "Click me"]   ;; <button>Click me</button>
```

## Attributes

Attributes are a map in the second position. Keyword colons on keys are stripped automatically:

```scheme
[:div {:id "main" :class "container" :data-count "5"}
  [:p "Hello"]]
```

Renders as: `<div id="main" class="container" data-count="5"><p>Hello</p></div>`

### Style Attribute

Style accepts either a string or a map of CSS properties:

```scheme
;; String form
[:p {:style "color: red; font-size: 14px"} "Red text"]

;; Map form — property names are used as-is
[:p {:style {:color "red" :font-size "14px"}} "Red text"]
```

### Boolean Attributes

Boolean attributes are set or removed based on truthiness:

```scheme
[:input {:disabled true}]   ;; <input disabled>
[:input {:disabled false}]  ;; <input>  (attribute removed)
[:input {:checked true}]    ;; sets the checked DOM property
```

### DOM Properties

`value`, `checked`, and `disabled` set the corresponding DOM properties directly rather than using `setAttribute`:

```scheme
[:input {:type "text" :value "initial"}]
[:input {:type "checkbox" :checked true}]
```

## Event Handlers

Event handlers use `on-*` attributes. In SIP markup, the value must still be a **named function** string. The handler is installed as a delegated event via a `data-sema-on-*` attribute:

```scheme
(define (handle-click ev)
  (println "clicked!"))

[:button {:on-click "handle-click"} "Click me"]
```

The event handler receives a numeric event handle as its argument. Use `dom/event-value` to read `event.target.value` from it, or `dom/prevent-default!` to cancel the default action.

> **Gotcha**: Inline lambdas are not supported as SIP event handler values. The value must be a string naming a defined function. Lower-level APIs like `dom/on!` can accept function values, but SIP delegated event attributes are still name-based.

## Children

Children can be strings, numbers, booleans, `nil`, or nested SIP vectors:

```scheme
[:div
  [:h1 "Title"]
  [:p "Paragraph " 42 " items"]
  [:p (if logged-in? "Welcome" "Please log in")]]
```

`nil` renders as an empty text node.

## Fragments

When the first element of an array is not a string (keyword), the array is treated as a fragment -- a list of sibling elements:

```scheme
;; Returns two paragraphs as siblings
[[:p "First"] [:p "Second"]]
```

This is useful for returning multiple root elements from a function.

## Conditional Rendering

Use standard Sema conditionals -- they return SIP vectors:

```scheme
(if loading?
  [:div {:class "spinner"} "Loading..."]
  [:div {:class "content"} "Ready"])
```

## List Rendering

Use `map` to produce lists of elements:

```scheme
[:ul
  (map (fn [item] [:li (:text item)]) items)]
```

Since `map` returns a list (not a keyword-prefixed vector), the result is treated as a fragment and each element is appended.

## Rendering Functions

### `sip/render`

Renders SIP data and returns an element handle (numeric ID):

```scheme
(def el (sip/render [:div {:class "card"} "Hello"]))
(dom/append-child! parent el)
```

Non-element nodes (text, fragments) are wrapped in a `<span>`.

### `sip/render-into!`

Renders SIP data into a target element selected by CSS selector. Replaces existing content:

```scheme
(sip/render-into! "#app"
  [:div
    [:h1 "My App"]
    [:p "Welcome"]])
```

### DOM Aliases

`dom/render` and `dom/render-into!` are identical to their `sip/` counterparts.

## Gotchas

- **Keyword colons are stripped** from both tag names and attribute keys. `:div` becomes `div`, `:class` becomes `class`.
- **SIP event handlers must be named functions** -- you cannot pass a lambda directly in `{:on-click ...}`. Define the function first, then reference it by name as a string.
- **`hiccup/render` and `hiccup/render-into!`** are legacy aliases for backward compatibility. Prefer the `sip/` or `dom/` namespace.
