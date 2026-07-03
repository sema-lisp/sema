---
outline: [2, 3]
---

# Sema Web

## What is Sema Web?

Sema Web embeds the Sema language in web pages via WebAssembly. Think of it like Lua for game engines, but for the browser: a small, expressive scripting language with reactive state, declarative markup, and direct DOM access.

The `@sema-lang/sema-web` package gives you:

- **Reactive state** -- signals, computed values, watchers, and batched updates
- **SIP markup** -- vectors-as-DOM, a compact alternative to HTML templates
- **Components** -- auto-re-rendering views driven by signal dependencies
- **DOM bindings** -- query, create, mutate elements from Sema code
- **Storage, routing, CSS, HTTP, LLM** -- browser APIs exposed as Sema functions

Everything runs client-side. No build step required -- add a script tag and go.

## Quick Example

A counter in 12 lines of Sema:

```scheme
;; Reactive state
(def count (state 0))

;; Event handler
(define (increment ev)
  (update! count (fn (n) (+ n 1))))

;; Component
(defcomponent counter-view ()
  [:div {:class "counter"}
    [:p "Count: " @count]
    [:button {:on-click "increment"} "+"]])

;; Mount to the DOM
(mount! "#app" "counter-view")
```

Embed it in an HTML page:

```html
<!DOCTYPE html>
<html>
<body>
  <div id="app"></div>

  <script type="module">
    import { SemaWeb } from "@sema-lang/sema-web";
    await SemaWeb.init();
  </script>

  <script type="text/sema">
    (def count (state 0))

    (define (increment ev)
      (update! count (fn (n) (+ n 1))))

    (defcomponent counter-view ()
      [:div {:class "counter"}
        [:p "Count: " @count]
        [:button {:on-click "increment"} "+"]])

    (mount! "#app" "counter-view")
  </script>
</body>
</html>
```

## Architecture

```
Browser Page
  │
  ├── <script type="module">
  │     SemaWeb.init()
  │       │
  │       ├── Load WASM binary (sema-wasm)
  │       ├── Create interpreter instance
  │       ├── Register JS bindings:
  │       │     dom/*    store/*    console/*
  │       │     state    put!      update!
  │       │     computed batch     watch
  │       │     mount!   local     on-mount
  │       │     sip/*    router/*  css/*
  │       │     http/*   llm/*
  │       └── Auto-discover <script type="text/sema"> tags
  │
  ├── <script type="text/sema">
  │     Sema code evaluated by WASM interpreter
  │       │
  │       ├── state/put!/update!  →  @preact/signals-core
  │       ├── SIP markup          →  renderSip() → DOM nodes
  │       ├── mount!              →  effect() + morphdom diffing
  │       └── on-click etc.       →  delegated event handling
  │
  └── DOM
        Patched efficiently via morphdom (no full replacement)
```

**Key design decisions:**

| Decision | Rationale |
| --- | --- |
| TypeScript wrapping WASM | DOM mutations (~5ms) dominate cost, not WASM boundary (~0.5ms). TS gives npm ecosystem access. |
| `@preact/signals-core` | 1.6kB gzipped, battle-tested reactivity with auto-tracking. |
| `morphdom` for diffing | 3kB gzipped, preserves focus/scroll, no virtual DOM overhead. |
| Event delegation | One listener per event type on the mount root. Survives morphdom patches. |
| Named local state | `(local "name" 0)` keyed by name, not call order. No hooks rules. |

## Core Concepts

### SIP Markup (Sema Interface Primitives)

Components return vectors that describe DOM structure -- Sema's take on Clojure's Hiccup:

```scheme
[:div {:class "card"}
  [:h1 "Title"]
  [:p {:style "color: gray"} "Subtitle"]]
```

The name is a nod: a sip cures a hiccup.

### Reactive State

State is managed through signals. Reading a signal inside a component or `computed` expression automatically subscribes to changes:

```scheme
(def name (state "world"))
(def greeting (computed (string-append "Hello, " @name "!")))
```

When `name` changes, `greeting` recomputes, and any component reading either value re-renders.

### Components

Components are functions that return SIP markup. Mount them to a DOM element and they auto-re-render when their signal dependencies change:

```scheme
(defcomponent app ()
  [:div [:p @greeting]])

(mount! "#app" "app")
```

Re-rendering uses morphdom for efficient DOM patching -- only changed nodes are touched, and focused inputs keep their state.

## Feature Modules

| Module | Namespace | Purpose |
| --- | --- | --- |
| [Reactive State](./reactive-state) | `state`, `put!`, `update!`, `computed`, `batch`, `watch` | Signal-based reactivity |
| [Components](./components) | `defcomponent`, `mount!`, `local`, `on-mount` | Reactive UI components |
| [SIP Markup](./sip-markup) | `sip/*` | Declarative DOM rendering |
| [DOM API](./dom-api) | `dom/*` | Low-level DOM manipulation |
| [Store](./store) | `store/*` | localStorage / sessionStorage |
| [Routing](./routing) | `router/*` | Hash-based SPA routing |
| [Scoped CSS](./css) | `css/*` | Dynamic style injection |
| [HTTP & Streams](./http) | `http/*` | Browser fetch integration and SSE streams |
| [LLM](./llm) | `llm/*` | AI completions via proxy |

## Next Steps

- [Getting Started](./getting-started) -- install, set up a page, evaluate code
- [Building a Sema Web App](./building-apps) -- recommended project layout and compiled-archive workflow
- [Reactive State](./reactive-state) -- the core programming model
- [Components](./components) -- building interactive UIs
