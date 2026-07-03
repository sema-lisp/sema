# @sema-lang/sema-web

Sema as an embedded web scripting language — use Sema to build interactive web UIs with DOM bindings, persistent storage, and `<script type="text/sema">` support.

> Built on top of [`@sema-lang/sema`](https://www.npmjs.com/package/@sema-lang/sema), the WebAssembly-powered Sema interpreter.

## Quick Start

Add Sema to any HTML page:

```html
<!DOCTYPE html>
<html>
<body>
  <div id="app"></div>

  <script type="text/sema">
    ;; Create a greeting element
    (let ((el (dom/create-element "h1")))
      (dom/set-text! el "Hello from Sema!")
      (dom/set-style! el "color" "#6366f1")
      (dom/append-child! (dom/query "#app") el))
  </script>

  <script type="module">
    import { SemaWeb } from "@sema-lang/sema-web";
    await SemaWeb.init();
  </script>
</body>
</html>
```

## Installation

```bash
npm install @sema-lang/sema-web
```

Or use from a CDN:

```html
<script type="module">
  import { SemaWeb } from "https://cdn.jsdelivr.net/npm/@sema-lang/sema-web/dist/index.js";
  await SemaWeb.init();
</script>
```

## Usage

### Auto-load `<script type="text/sema">` tags

The simplest way — `SemaWeb.init()` discovers and evaluates all Sema script tags:

```html
<script type="text/sema">
  (println "Hello from Sema!")
</script>

<script type="text/sema" src="app.sema"></script>

<script type="module">
  import { SemaWeb } from "@sema-lang/sema-web";
  await SemaWeb.init();
</script>
```

### Manual evaluation

Create an instance and evaluate code programmatically:

```js
import { SemaWeb } from "@sema-lang/sema-web";

const web = await SemaWeb.create({ autoLoad: false });

// Evaluate Sema code with DOM access
web.eval('(dom/set-text! (dom/query "#greeting") "Hello!")');

// Register custom JS functions callable from Sema
web.registerFunction("get-timestamp", () => Date.now());
web.eval("(console/log (get-timestamp))");
```

### External `.sema` files

Reference external Sema files with the `src` attribute:

```html
<script type="text/sema" src="counter.sema"></script>
```

### Production `.vfs` archives

For development and simple embeds, loading `.sema` source directly is fine. For production,
build a compiled `.vfs` archive and load that with the same script-tag API:

```bash
sema build --target web app.sema -o public/app.vfs
```

```html
<script type="text/sema" src="/app.vfs"></script>
<script type="module">
  import { SemaWeb } from "@sema-lang/sema-web";
  await SemaWeb.init();
</script>
```

`SemaWeb.init()` auto-detects `.vfs` archives and runs their compiled `__main__.semac` entry
instead of evaluating source in the browser.

## API Namespaces

### `dom/*` — DOM Manipulation

```sema
;; Query elements
(dom/query "#app")              ;; → element handle or nil
(dom/query-all ".item")         ;; → list of element handles
(dom/get-id "my-element")       ;; → element handle or nil

;; Create elements
(dom/create-element "div")      ;; → element handle
(dom/create-text "Hello")       ;; → text node handle

;; Tree manipulation
(dom/append-child! parent child)
(dom/remove-child! parent child)
(dom/remove! element)

;; Attributes
(dom/set-attribute! el "class" "container")
(dom/get-attribute el "href")
(dom/remove-attribute! el "disabled")

;; CSS classes
(dom/add-class! el "active" "visible")
(dom/remove-class! el "hidden")
(dom/toggle-class! el "open")
(dom/has-class? el "active")    ;; → #t or #f

;; Styles
(dom/set-style! el "color" "red")
(dom/get-style el "color")

;; Content
(dom/set-text! el "Hello")
(dom/get-text el)
(dom/set-html! el "<b>Bold</b>")

;; Form values
(dom/set-value! input "text")
(dom/get-value input)

;; Events
(dom/on! el "click" my-handler)
;; or:
(dom/on! el "click" "my-handler")
(dom/off! el "click" my-handler)
(dom/prevent-default! event)
```

### `store/*` — Persistent Storage

```sema
;; localStorage
(store/set! "key" "value")
(store/get "key")               ;; → value or nil
(store/remove! "key")
(store/clear!)
(store/keys)                    ;; → list of keys
(store/has? "key")              ;; → #t or #f

;; sessionStorage
(store/session-set! "key" "value")
(store/session-get "key")
(store/session-remove! "key")
(store/session-clear!)
```

### `http/*` — Browser HTTP & Streams

Sema Web uses the standard `http/*` request functions for browser `fetch`, and adds a
streaming SSE API for long-lived responses.

```sema
;; Standard requests
(http/get "/api/posts")
(http/post "/api/messages" {:text "hello"})

;; Streaming SSE connection
(def stream
  (http/event-source
    {:url "/api/events"
     :headers {"authorization" "Bearer demo-token"}
     :with-credentials true}))

;; Read current stream state
(:data @stream)
(:event @stream)
(:done @stream)
(:error @stream)

;; Close when finished
(http/close-stream stream)
```

`http/event-source` uses a fetch-based SSE client rather than the browser's native
`EventSource`, so it supports headers, credentials, and POST bodies. Streams created in
components are automatically closed on unmount.

### `console/*` — Browser Console

```sema
(console/log "message" value)
(console/warn "warning!")
(console/error "error!")
(console/info "info")
(console/debug "debug")
(console/clear)
(console/time "label")
(console/time-end "label")
```

### Reactive State

Reactive state is built around signals.

```sema
;; Create a signal
(def count (state 0))

;; Read value (tracks dependency in reactive context)
@count                       ;; → 0

;; Set value directly
(put! count 42)

;; Update by applying a function to current value
(update! count (fn (n) (+ n 1)))
```

Use `(watch signal callback)` to observe changes and `(unwatch! watch-id)` to dispose a watch.

### SIP — Declarative DOM

Describe UI as data using vectors and maps (the hiccup convention):

```sema
;; Hiccup format: [:tag {:attr "value"} ...children]

[:div {:class "card"}
  [:h1 "Hello"]
  [:p {:style "color: blue"} "World"]
  [:button {:on-click "handle-click"} "Click me"]]
```

**Attributes:**
- `class` — sets className
- `style` — CSS string or property map: `{:color "red" :font-size "14px"}`
- `on-*` — SIP delegated event handlers use a Sema function name string: `{:on-click "my-handler"}`
- `value`, `checked`, `disabled` — form element properties
- All other attributes use `setAttribute`

**Standalone rendering:**

```sema
;; Render SIP to an element handle
(define el (sip/render [:div {:class "box"} "hello"]))
(dom/append-child! (dom/query "#app") el)

;; Render directly into a target element
(sip/render-into! "#app" [:h1 "Hello from Sema!"])
```

### Components — Reactive Rendering

Define a component as a function returning SIP, then mount it to a DOM element.
The component **automatically re-renders** when signals it reads during render change.

```sema
;; State
(def count (state 0))

;; Event handlers
(define (increment ev)
  (update! count (fn (n) (+ n 1))))

;; Component: a function that returns SIP
(defcomponent counter-view ()
  [:div
    [:h1 @count]
    [:button {:on-click "increment"} "+"]])

;; Mount to DOM
(mount! "#app" "counter-view")
```

**How it works:**
1. `mount!` calls the component function
2. During the call, it tracks which signals are read
3. It renders the returned SIP to DOM
4. When any tracked signal changes, the component re-renders automatically
5. Multiple updates in the same tick are batched

**Component functions:**
- `(mount! selector fn-name)` — mount a component to a CSS selector
- `(component/unmount! selector)` — remove a mounted component
- `(component/force-render! selector)` — force re-render

### `llm/*` — LLM Proxy

LLM functions are available in the browser when a proxy URL is configured. The proxy server holds
API keys and forwards requests to the actual LLM providers (OpenAI, Anthropic, etc.).

```js
// Enable LLM in the browser
const web = await SemaWeb.create({
  llmProxy: "https://api.example.com/llm",
});
```

```sema
;; Simple completion
(llm/complete "Say hello in exactly 5 words" {:max-tokens 50})

;; Chat with messages
(llm/chat
  (list (message :system "You are a helpful assistant.")
        (message :user "What is Sema?"))
  {:model "gpt-4o" :max-tokens 200})

;; Structured extraction
(llm/extract
  {:name {:type "string"} :age {:type "number"}}
  "John is 30 years old")

;; Classification
(llm/classify (list "positive" "negative" "neutral")
  "This product is amazing!")

;; Text embeddings
(llm/embed "Hello world")

;; List available models from the proxy
(llm/list-models)
```

**Proxy protocol:**

The proxy server must implement these POST endpoints:

| Endpoint | Body | Returns |
|----------|------|---------|
| `/complete` | `{prompt, model?, max-tokens?, ...}` | `{content, usage?}` or string |
| `/chat` | `{messages, model?, max-tokens?, ...}` | `{content, usage?}` or string |
| `/extract` | `{schema, text, model?, ...}` | extracted data object |
| `/classify` | `{categories, text, model?, ...}` | `{category}` or string |
| `/embed` | `{text, model?, ...}` | `{embedding: [...]}` or `[...]` |
| `/models` (GET) | — | `{models: [...]}` |
| `/stream` | `{messages, model?, max-tokens?, ...}` | normalized SSE: `token`, `done`, `error` events |

On errors, the proxy should return an appropriate HTTP status code (4xx/5xx).
The response body is surfaced in the Sema error message.

**Authentication:**

The `token` option sends a `Bearer` token on each request (for authenticating the
browser client to your proxy — never send LLM API keys to the browser):

```js
await SemaWeb.create({
  llmProxy: {
    url: "https://api.example.com/llm",
    token: "user-session-jwt",
    timeout: 30000,
  },
});
```

## Configuration

```js
const web = await SemaWeb.create({
  // Auto-discover <script type="text/sema"> tags (default: true)
  autoLoad: true,

  // Register dom/* functions (default: true)
  dom: true,

  // Register store/* functions (default: true)
  store: true,

  // Register console/* functions (default: true)
  console: true,

  // Register reactive bindings (default: true)
  reactive: true,

  // Register SIP rendering bindings (default: true)
  sip: true,

  // Register component/mount system (default: true)
  // Automatically enables reactive + sip
  components: true,

  // LLM proxy — enables llm/* functions in the browser
  // Simple: just the URL
  llmProxy: "https://api.example.com/llm",
  // Or full options:
  // llmProxy: {
  //   url: "https://api.example.com/llm",
  //   token: "user-session-token",
  //   timeout: 30000,
  // },

  // Custom WASM URL (for CDN deployment)
  wasmUrl: "https://cdn.example.com/sema_wasm_bg.wasm",

  // Sandbox capabilities to deny
  deny: ["network"],
});
```

## Example: Interactive Counter

### Imperative style (dom/* only)

```sema
;; counter.sema — A simple click counter

;; State
(define count 0)

;; Create UI elements
(let ((container (dom/query "#app"))
      (display (dom/create-element "h1"))
      (btn-inc (dom/create-element "button"))
      (btn-dec (dom/create-element "button")))

  ;; Set initial content
  (dom/set-text! display "0")
  (dom/set-text! btn-inc "+")
  (dom/set-text! btn-dec "−")

  ;; Style
  (dom/set-style! display "font-size" "4rem")
  (dom/set-style! display "text-align" "center")

  ;; Append to container
  (dom/append-child! container display)
  (dom/append-child! container btn-inc)
  (dom/append-child! container btn-dec)

  ;; Store element handles for event handlers
  (define display-el display)
  (define inc-btn btn-inc)
  (define dec-btn btn-dec))

;; Event handlers
(define (on-increment evt)
  (set! count (+ count 1))
  (dom/set-text! display-el (number->string count)))

(define (on-decrement evt)
  (set! count (- count 1))
  (dom/set-text! display-el (number->string count)))

;; Bind events
(dom/on! inc-btn "click" "on-increment")
(dom/on! dec-btn "click" "on-decrement")
```

### Reactive style (state + SIP + mount!)

```sema
;; counter-reactive.sema — Reactive counter with automatic re-rendering

;; State
(def count (state 0))

;; Event handlers
(define (handle-increment ev)
  (update! count (fn (n) (+ n 1))))

(define (handle-decrement ev)
  (update! count (fn (n) (- n 1))))

(define (handle-reset ev)
  (put! count 0))

;; Component — returns SIP, re-renders when state changes
(defcomponent counter-view ()
  [:div {:class "counter"}
    [:h2 "Sema Reactive Counter"]
    [:div {:class "display"} @count]
    [:div {:class "buttons"}
      [:button {:on-click "handle-decrement"} "−"]
      [:button {:on-click "handle-reset"} "Reset"]
      [:button {:on-click "handle-increment"} "+"]]])

;; Mount — binds view to DOM, auto-re-renders on state change
(mount! "#app" "counter-view")
```

## Architecture

```
┌─────────────────────────────────────────┐
│  HTML Page                              │
│                                         │
│  <script type="text/sema">              │
│    (mount! "#app" "my-view")            │
│    (llm/chat messages opts)             │
│  </script>                              │
│                                         │
├─────────────────────────────────────────┤
│  @sema-lang/sema-web                    │
│  ┌──────────┬──────────┬──────────────┐ │
│  │ dom/*    │ store/*  │ console/*    │ │
│  │ bindings │ bindings │ bindings     │ │
│  └──────────┴──────────┴──────────────┘ │
│  ┌──────────┬──────────┬──────────────┐ │
│  │ state    │ sip/*    │ component/*  │ │
│  │ put!/…   │ render   │ mount!       │ │
│  └──────────┴──────────┴──────────────┘ │
│  ┌────────────────────────────────────┐ │
│  │ llm/* proxy (→ backend server)    │ │
│  └────────────────────────────────────┘ │
│  ┌────────────────────────────────────┐ │
│  │ Script loader (<script> discovery) │ │
│  └────────────────────────────────────┘ │
├─────────────────────────────────────────┤
│  @sema-lang/sema (interpreter API)      │
├─────────────────────────────────────────┤
│  @sema-lang/sema-wasm (WASM VM)        │
└─────────────────────────────────────────┘
         │
         ▼  (when llmProxy configured)
┌─────────────────────────────────────────┐
│  Your LLM Proxy Server                  │
│  Holds API keys, forwards to providers  │
│  → OpenAI / Anthropic / Gemini / etc.   │
└─────────────────────────────────────────┘
```

`sema-web` uses the `registerFunction` API from `@sema-lang/sema` to bridge JavaScript browser APIs into the Sema interpreter. No Rust code changes are required — all DOM, storage, and console bindings are implemented as JavaScript callbacks registered into the interpreter at initialization.

## License

MIT
