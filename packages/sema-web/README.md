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

The package is ESM-only. It targets browsers and bundlers, and the interpreter
it wraps (`@sema-lang/sema`) is ESM-only too, so there is no CommonJS build to
`require()` — on Node 22.12 or newer `require("@sema-lang/sema-web")` still
works through Node's own `require(esm)` support; older Node needs
`await import(...)`. TypeScript consumers want `"module": "nodenext"`.

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
(dom/event-current-target event)   ;; → the element that declared the handler

;; Forms
(dom/event-form-data event)        ;; → {:title "hi" :tag ("a" "b")} or nil
(dom/form-data el)                 ;; → the same, from a form (or any element in one)
(dom/event-form event)             ;; → the owning <form> handle or nil

;; Checkbox / radio / select
(dom/event-checked event)          ;; → #t / #f / nil
(dom/checked? el)                  ;; → #t or #f
(dom/selected-values select-el)    ;; → ("s" "l")
(dom/event-selected-values event)  ;; → the same, from an event
```

A field name that occurs once is a string; a repeated one is a list. File
inputs come back as `{:name … :size … :type …}`. Unchecked, disabled, and
unnamed controls are absent, as in a real submission.

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

### `resource` — Async Data in a Signal

A resource wraps one request in a reactive signal, so a component renders
loading, error, and success from a single dereference and never touches a
promise.

```sema
(defcomponent user-card (props)
  (def u (resource "user" (fn () (string-append "/api/users/" (:id props)))))

  (cond
    ((:loading @u) [:p "Loading..."])
    ((:error @u)   [:p {:class "error"} (:error @u)])
    (else          [:h2 (:name (:value @u))])))
```

The spec function **describes** the request — a URL string, or a map with
`:url`, `:method`, `:headers`, `:body`, `:with-credentials`, `:as` — and the
runtime performs it: every `http/*` native rejects on the synchronous path a
host-invoked Sema callback always runs on. Dereferencing gives
`{:loading :value :error :status}`, and a refetch keeps the previous `:value`
and `:status` on screen rather than blanking the UI.

A resource **refetches when the request its spec resolves to changes**. The spec
is re-resolved on a clean stack whenever the owning component re-renders, and a
moved URL, method, header, or body starts a fresh attempt; an identical request
does nothing. The comparison is on the request and never on the closure, since a
render allocates a new closure every time. A spec reading state the view does
not read has nothing to re-render it — read the value in the view, or refresh
explicitly with `(effect (list @uid) (fn () (resource/refresh! "user")))`.

```sema
(resource/refresh! "user")   ;; revalidate, keeping the current value
(resource/cancel! "user")    ;; abort the in-flight attempt; not a failure
```

`(resource "name" …)` is memoized per component instance — including per
composed child — which is what stops "response writes signal → re-render → new
resource → new request" from looping. The unnamed form `(resource spec-fn)` is
for module top level; inside a component (render, effect body, event handler,
timer) it is rejected, because each of those runs again and would allocate a new
request and signal every time. Unmounting aborts the request and releases
everything it owned.

Full documentation: [Async Resources](https://sema-lang.com/docs/web/resources).

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

`watch` and `computed` belong somewhere that runs once — module top level,
`on-mount`, or an `effect` body. Called from a component's **render body** they
run again on every render, so the runtime keeps them bounded: a watch is
memoized per render site (one subscription, callback swapped each render, and
one `onerror` note per component), and a computed is disposed and rebuilt so its
value always matches the render that read it.

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
- `on-*` — SIP delegated event handlers use a Sema function name string: `{:on-click "my-handler"}`, optionally with dotted modifiers: `.prevent`, `.stop`, `.once`, `.capture`, `.self` (`{:on-submit.prevent "save"}`, `{:on-click.stop.once "buy"}`). `.prevent` runs before the handler — and keeps running after a `.once` is spent, so a `.prevent.once` form never navigates — while `.stop` runs after it. An unknown modifier **or an event name the delegator does not listen for** is reported through `onerror` and the handler is not installed: `{:on-sumbit.prevent "save"}` would otherwise render an attribute that can never fire, and the form would navigate away with no signal at all. For an event outside the delegated set (a custom element's, or a non-bubbling one like `scroll`), attach it with `dom/on!` from an `on-mount` callback
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
- `(component/force-render! selector)` — force re-render. Refused and reported
  as `force-render:<component>` if that component is already rendering, so it
  belongs in an event handler, not in a render or effect body.

**Lifecycle (inside a component body):**
- `(local name initial)` — component-scoped state, keyed by name
- `(on-mount fn)` — run once after the first render; may return a cleanup
- `(effect deps fn)` — run after render, re-run when `deps` change; may return a
  cleanup that runs before each re-run and at teardown. `deps` is a list —
  `(list)` for "once", `nil` for "every render" — compared structurally.
- `(on-unmount fn)` — run once at teardown

```sema
(defcomponent clock ()
  (let ((now (local "now" 0)))
    (effect (list)
      (fn ()
        (let ((id (js/set-interval (fn () (put! now (+ @now 1))) 1000)))
          (fn () (js/clear-interval id)))))
    [:time (number->string @now)]))
```

Effects and `on-unmount` are matched across renders by **call order**, so
register them unconditionally at the top level of the body; a render that
changes the sequence is reported as `lifecycle:<component>#<slot>`, and an
`on-unmount` hook a render stops registering never runs (hooks run at teardown
only, never because a slot was re-keyed).

Everything an effect creates — intervals, watches, streams, state — is owned by
the component and disposed **with the component**. That is a teardown
guarantee, not a re-run guarantee: an effect whose deps change must return a
cleanup that undoes what its body created, or the next run adds a second
interval/watch alongside the first. Errors in a body or a cleanup are reported
through [`onerror`](#onerror) without aborting teardown.

### `router/*` — SPA Routing

The router keeps the current route in a signal, so a component that reads it
re-renders on navigation like any other reactive value.

```sema
(router/init!
  {:mode :hash                    ;; :hash (default) or :history
   :not-found "missing-page"      ;; handler for an unmatched path
   :scroll-to-top true            ;; scroll to the top on navigation
   :focus "#main"                 ;; move focus there on navigation
   :routes {"/" "home-page"
            "/todos" "todo-list-page"
            "/todos/:id" "todo-detail-page"}})

(defcomponent app-view ()
  (let ((r (router/current-route)))
    [:main {:id "main"}
     [:nav (router/link "/todos" "Todos" {:class "nav-link"})]
     (cond ((equal? (:handler r) "home-page") (component/render home-page {:route r}))
           ((equal? (:handler r) "todo-detail-page") (component/render todo-detail-page {:route r}))
           (else (component/render missing-page {:route r})))]))
```

**Functions:**
- `(router/init! routes-or-options)` — register routes. Accepts either a bare
  `{pattern handler}` map or the options map above; a `:routes` key holding a
  *map* is read as options, while a `:routes` key holding a handler name is the
  ordinary route `/routes`.
- `(router/current-route)` — `{:path "/todos/42" :params {:id "42"} :query {:tab "open"} :handler "todo-detail-page"}`,
  or `nil` when nothing matched and no `:not-found` handler is registered
- `(router/push! path)` — navigate, adding a history entry
- `(router/replace! path)` — navigate without adding one
- `(router/back!)` — go back
- `(router/link path label attrs)` — SIP data for an accessible anchor
- `(router/href path)` — the `href` a link to `path` needs in the active mode
- `(router/current)` — the route signal id, for `deref`/`watch`

**Routes and paths:**
- `:id`-style segments become `:params`, percent-decoded.
- The query string is parsed into `:query`: `?tab=open&tag=a&tag=b` reads as
  `{:tab "open" :tag ("a" "b")}` — a repeated key collects into a list, a key
  with no `=` gets `""`, and `+` decodes to a space. A malformed escape keeps its
  raw text rather than failing the route.
- Patterns match the **path only**. A `?` in a pattern starts a query string
  there, too, and is ignored.
- Paths are normalized to a leading `/`, so `"todos"` and `"/todos"` are one
  route.
- A pattern may hold any character a URL segment can. `{"/søk" "search-page"}`
  matches even though the browser stores that URL percent-encoded
  (`#/s%C3%B8k`), because each literal character is compiled to accept both
  forms.

**Links:** `router/link` renders an `<a>` whose clicks are intercepted, so
navigation never reloads the page, and adds `aria-current="page"` when it points
at the current path. Modified clicks (cmd/ctrl/shift/alt), middle clicks,
`:target`, and `:download` are left to the browser. A path that leaves the app
(`https://…`, `//host`, `javascript:`, and the backslash spellings `/\host`,
`\\host`, `\host` that a browser resolves to the same cross-origin URL) is
refused: the link renders as an inert `<span>` and the failure is reported
through [`onerror`](#onerror). `router/push!`, `router/replace!` and
`router/href` admit paths by the same rule.

`:mode :history` uses real paths via `pushState`, which requires the host server
to serve the app shell for every route. `:hash` needs nothing from the host and
is the default.

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
    headers: { "X-Client": "my-app" },
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
  router: true,       // router/* namespace
  css: true,          // css/* scoped styles
  http: true,         // http/event-source (SSE)
  resources: true,    // resource, resource/refresh!, resource/cancel!
  websocket: true,    // ws/* namespace

  // LLM proxy — enables llm/* functions in the browser
  // Simple: just the URL
  llmProxy: "https://api.example.com/llm",
  // Or full options:
  // llmProxy: {
  //   url: "https://api.example.com/llm",
  //   token: "user-session-token",
  //   headers: { "X-Client": "my-app" },
  // },

  // Custom WASM URL (for CDN deployment)
  wasmUrl: "https://cdn.example.com/sema_wasm_bg.wasm",

  // Sandbox capabilities to deny
  deny: ["network"],

  // Application-level error hook (default: logs to console.error)
  onerror(error, context) {
    reportToSentry(error, { context });
  },

  // Dev mode: record a bounded event timeline (default: false)
  dev: false,
});
```

## Diagnostics and Dev Mode

### `onerror`

Every failure the runtime catches — a component render, a delegated event
handler, an effect cleanup, a stream, a script load — is routed to one hook:

```js
const web = await SemaWeb.create({
  onerror(error, context) {
    // context names the source: "component:todo-list",
    // "listener-cleanup:click", "inline-script:2", "event-source-cleanup"
    reportToSentry(error, { tags: { context } });
  },
});
```

Installing your own handler replaces the `console.error` default. It does not
disable diagnostics recording — entries are captured first, then your handler
runs, so a custom reporter and the dev timeline coexist.

### `dev`

Dev mode records a bounded timeline of what the runtime is doing:

```js
const web = await SemaWeb.create({ dev: true });

web.diagnostics.all();            // every retained entry, oldest first
web.diagnostics.byKind("error");  // just the failures
web.diagnostics.byKind("route");  // navigation history
web.diagnostics.slowRenders();    // renders at/over the threshold
web.diagnostics.dropped;          // entries evicted by the size bound
```

Recorded kinds:

| Kind | Recorded when |
| --- | --- |
| `error` | any failure reaching the error hook |
| `render` | a component finishes rendering (carries `durationMs`) |
| `route` | the route changes, including "no match" |
| `stream` | an SSE or LLM stream opens or closes |
| `script` | a `<script type="text/sema">` loads, evaluates, or fails |

Tune it with an object:

```js
await SemaWeb.create({
  dev: {
    limit: 500,        // ring size (default: 100)
    slowRenderMs: 8,   // slow-render threshold (default: 16, one 60fps frame)
    overlay: true,     // mount the on-page dev panel (default: false)
  },
});
```

**Dev mode is off by default and free when off.** Entries are built lazily, so
with `dev` unset nothing is allocated on the render path — no object, no string
interpolation. The ring is bounded and reports what it evicted, so a component
stuck in a render loop cannot grow it without limit.

The overlay lives in its own module, pulled in by a dynamic `import()` only
when `overlay: true`, so it is a separate chunk (`dist/devtools-*.js`) that a
production bundle never loads.

`web.dispose()` removes the overlay and drops every diagnostics subscriber; the
recorded entries stay readable, which is usually when you most want them.

## Testing Components

`@sema-lang/sema-web/testing` mounts a component into JSDOM with a **real**
interpreter and the full set of bindings, then hands back helpers for driving
and inspecting it:

```js
import { renderSema, disposeAllScreens } from "@sema-lang/sema-web/testing";

afterEach(() => disposeAllScreens());

it("counts up", async () => {
  const screen = await renderSema(
    `(def count (state 0))
     (defcomponent view () [:button {:on-click "inc"} (str @count)])
     (define (inc ev) (update! count (fn (n) (+ n 1))))`,
    { mount: "view" },
  );

  screen.click("button");

  expect(screen.text("button")).toBe("1");
  expect(screen.errors).toEqual([]);
  screen.unmount();
  expect(screen.leaks()).toEqual({});
});
```

Requires the `jsdom` test environment (Vitest: `environment: "jsdom"`) and a
Node test runner — the helper reads the WASM binary off disk.

Pass `{ url: "#/todos/42" }` (or a history-mode `"/todos/42?tab=open"`) to boot
a screen on a route. Every screen resets the location, to `/` when `url` is
omitted: JSDOM shares one `window` across a test file, so without that a screen
that navigated would decide which route the next one mounts on.

### What the screen gives you

| Group | Members |
| --- | --- |
| Markup | `html()`, `text()`, `find()`, `query()`, `findAll()` — all scoped to the mount container |
| Events | `click()`, `fill()`, `select()`, `check()`, `submit()`, `press()`, `fire()`, `focus()` |
| Sema | `eval()` (raw result, never throws), `run()` (value, throws the interpreter's message), `output` |
| State | `signal(name)`, `setSignal(name, value)` |
| Lifecycle | `mount()`, `unmount()`, `flush()`, `dispose()` |
| Failures | `errors`, `errorContexts()`, `diagnostics` |
| Cleanup | `leaks()`, `snapshot()` |

`renderSema` replaces the runtime's `console.error` default with a collector, so
every caught failure lands in `screen.errors` as `{error, context, message}`
instead of in your test output. Dev mode is **on** by default, so
`screen.diagnostics` has a render timeline and dev-only checks (duplicate SIP
keys) report; pass `{ web: { dev: false } }` for a production-shaped run.

### Proving nothing leaked

`leaks()` returns only the registries that grew — `{}` means clean, and a
failure names what is still live:

```js
screen.unmount();
expect(screen.leaks()).toEqual({});   // → e.g. { intervals: 1 } when it is not
```

It counts handles, signals, listeners, watches, intervals, streams, resources,
sockets, cleanup hooks, mounted components, and lifecycle slots, relative to a
baseline taken **after** `source` was evaluated and **before** anything was
mounted. Module-level state a fixture defines is therefore the app's, not the
component's, and does not read as a leak. Call it after `unmount()`: that is
what proves component teardown. After `dispose()` the whole context is
force-drained, so a clean report there says much less.

### Options

```js
await renderSema(source, {
  mount: "view",              // component to mount (omit to only evaluate source)
  props: { title: "Hello" },  // ":title" keys work too
  target: "#app",             // mount selector
  html: '<div id="app"></div>',
  web: { dev: false },        // merged over the harness defaults
  onerror(error, context) {}, // also forward captured errors here
  wasmUrl: "...",             // or set SEMA_WASM_PATH
});
```

Gate a suite with `semaWasmAvailable()` so a checkout without the WASM build
skips rather than failing:

```js
import { semaWasmAvailable } from "@sema-lang/sema-web/testing";
const describeWithSema = semaWasmAvailable() ? describe : describe.skip;
```

The testing utilities are a **separate entry point** and are never imported by
the runtime, so `node:fs` cannot reach a browser bundle.

### The suites check the VM they load

`renderSema` and every browser fixture boot the real Sema VM out of
`packages/sema-wasm/pkg`, which is gitignored and which no JS entry point
builds. So both suites first compare that binary against a fingerprint of the
Rust sources in the tree, and fail with

```
packages/sema-wasm/pkg/sema_wasm_bg.wasm was built from different Rust sources
than the ones in this tree.
Rebuild it from the repo root:  npm run build:wasm
```

rather than reporting green about a VM of unknown vintage. Any build refreshes
the fingerprint — `npm run build:wasm`, `jake wasm.build`, a raw `wasm-pack`, or
CI restoring its content-keyed cache — so nothing has to remember to stamp it.
A checkout with no WASM build at all only fails the browser suite; the vitest
suites that need the VM gate themselves with `semaWasmAvailable()` and skip.

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
