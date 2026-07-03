# Sema Web: Production-Quality Embedded Script Engine

## Context

Branch `copilot/implement-sema-as-embedded-script-engine` has a proof-of-concept for embedding Sema (a Lisp) in web pages via WASM. Two npm packages exist (`@sema-lang/sema-web`, `@sema-lang/llm-proxy`) but have critical architectural issues: no DOM diffing (full `innerHTML` replacement), handle memory leaks, sync/async mismatch, zero tests, module-level singletons breaking multi-instance, and missing features (derive, hooks, router, CSS, http). This plan fixes all issues and implements the full vision from GitHub issue #18.

**Approach confirmed**: TypeScript wrapping sema-wasm (not Rust web_sys). Research shows DOM mutations dominate cost (~5ms), not WASM boundary crossing (~0.5ms for 100 serializations). TS gives us npm ecosystem access and fast iteration.

**Key dependencies to add**: `@preact/signals-core` (1.6kB gzipped, ships TS types), `morphdom` (3kB gzipped, ships TS types). No `@types/*` needed.

**Markup naming**: The vectors-as-DOM pattern (`[:div {:class "app"} [:p "Hello"]]`) is called **SIP** — **S**ema **I**nterface **P**rimitives. A nod to Clojure's Hiccup (a sip cures a hiccup). The internal module is `sip.ts`, the rendering function is `renderSip()`. User-facing, there is no `sip/*` namespace — components just return SIP markup, and `dom/render` is the escape hatch for manual rendering outside components.

**Rust changes required**: Two small additions to support the reactive API:
1. Add `@` reader macro to `sema-reader` (lex `@expr` → `(deref expr)`)
2. ~~No `set!` override~~ — `set!` is a hard-wired special form that only rebinds env vars. Use `put!` instead for signal mutation.

---

## Phase 0: Package Infrastructure + Rust Prerequisites

The repo has no npm workspace configuration, and two Sema language features are missing. Must set this up first.

### 0.1 Root workspace setup

Create root `package.json` with npm workspaces (npm 7+ native, no `workspace:*` protocol needed):

```json
{
  "private": true,
  "workspaces": ["packages/*"],
  "scripts": {
    "build:wasm": "wasm-pack build crates/sema-wasm --target web --out-dir ../../packages/sema-wasm/pkg",
    "build": "npm run build --workspaces",
    "test": "npm test --workspaces"
  }
}
```

### 0.2 Fix `@sema-lang/sema-wasm` package name

Current state: `playground/pkg/` has `name: "sema-wasm"` but `packages/sema/src/index.ts` imports `@sema-lang/sema-wasm`. Create `packages/sema-wasm/package.json` pointing to the wasm-pack output with the correct scoped name, or add a `wasm-pack build` output dir config.

### 0.3 WASM binary for tests

Unit tests (Vitest + jsdom) that test pure JS logic (reactive, handles, sip, store) mock the `SemaInterpreterLike` interface — no WASM needed. Integration tests and E2E tests need real WASM:

- Build WASM once: `wasm-pack build crates/sema-wasm --target web`
- Vitest config alias: `"@sema-lang/sema-wasm"` → local pkg output
- jsdom doesn't support `WebAssembly.instantiateStreaming` — use Node's WASM support with `--experimental-wasm-modules` or load via `fs.readFile` + `WebAssembly.instantiate`
- E2E (Playwright): Vite serves `.wasm` files with correct `Content-Type: application/wasm`

**Files**: root `package.json` (new), `packages/sema-wasm/package.json` (new or fix)

### 0.4 Add `@` reader macro (Rust change: `crates/sema-reader/src/reader.rs`)

`@` is currently a lex error (`assert!(read("@").is_err())`). Add support:

1. Add `Token::Deref` variant to the token enum
2. In the lexer, when `@` is encountered, emit `Token::Deref`
3. In `parse_expr`, add dispatch: when `Token::Deref` is seen, read the next expression and wrap as `(deref <expr>)`
4. Register `deref` as a native function in `sema-web` (JS side) that reads signal `.value`
5. Update tests: remove the `assert!(read("@").is_err())` test, add positive tests for `@x`, `@(+ 1 2)`, etc.

This is a small, isolated change (~20 lines in reader.rs). The VM lowering pass treats `(deref x)` as a normal function call — no special VM support needed.

### 0.5 Verify `put!` and `update!` are not reserved names

`put!` and `update!` must NOT be in `SPECIAL_FORM_NAMES` (they aren't — only `set!`, `define`, `if`, `lambda`, `quote`, `quasiquote`, `begin`, `cond`, `let`, `let*`, `letrec`, `and`, `or`, `when`, `unless`, `do`, `case`, `match`, `defmacro`, `define-macro`, `try`, `catch`, `throw`, `import`, `export`, `defrecord`). Confirmed safe to define as regular functions via `evalStr`.

**Files**: `crates/sema-reader/src/reader.rs` (lexer + parser + tests)

---

## Phase 1: Foundation (Core Fixes + Testing Infrastructure)

All subsequent phases depend on this. Fixes the 6 critical architectural problems and sets up testing.

### 1.1 Instance-scoped state (`packages/sema-web/src/context.ts` — NEW)

Replace module-level singletons in `handles.ts`, `reactive.ts`, `component.ts`, and `dom.ts` (its `listeners` map) with an instance-scoped context object.

```ts
export class SemaWebContext {
  // Handles (moved from handles.ts module globals)
  handles: Map<number, Element | Text | Event>
  nextHandle: number = 1

  // Signals (moved from reactive.ts module globals)
  signals: Map<number, Signal<any>>
  nextSignalId: number = 1

  // Components (moved from component.ts module globals)
  mountedComponents: Map<string, MountedComponent>

  // Event delegation (see 1.5)
  eventDelegator: EventDelegator

  // DOM event listeners (moved from dom.ts module globals)
  listeners: Map<string, EventListener>

  // Error handler (configurable)
  onerror: (error: Error, context: string) => void
}
```

Every `register*Bindings()` function gains a `ctx: SemaWebContext` parameter. `SemaWeb.create()` creates one context per instance. Handle IDs start at 1 per instance — no cross-instance ID collision.

**Files**: `context.ts` (new), `index.ts`, `handles.ts`, `dom.ts`, `reactive.ts`, `component.ts`, `sip.ts`

### 1.2 Replace custom atoms with `@preact/signals-core` (`reactive.ts` — REWRITE)

Current: custom `Atom` type with `Map<string, watcher>`, manual `startTracking`/`stopTracking` stack.
New: `signal()`, `computed()`, `effect()`, `batch()` from `@preact/signals-core`.

**Critical design note**: signals-core tracks `.value` reads via a global tracking stack. When Sema code calls `@x` (which the reader expands to `(deref x)` — see Phase 0.4), the WASM interpreter synchronously calls the JS `deref` callback, which reads `signal.value`. Because this is a synchronous call on the same JS stack, signals-core's tracking context IS active during the read. This has been validated — effects track nested synchronous function calls regardless of depth. The invariant: the `deref` JS callback must ALWAYS read `signal.value` directly and synchronously, never cache or defer the read.

**Why `put!` not `set!`**: `set!` is a hard-wired special form in Sema's evaluator (`special_forms.rs`). It calls `require_symbol` on its first arg and mutates the environment binding. It cannot be overridden by `define` — the special form dispatch fires before any env lookup. `(set! count 42)` would destroy the signal ID stored in `count`, replacing it with `42`. `put!` has no collision — it's a regular function name.

#### Reactive API (Option D — web-framework vocabulary)

The Sema API uses universally understood naming instead of Clojure jargon. An LLM or developer who knows React/Vue/Solid/Svelte can read this code and infer semantics without documentation:

```sema
;; ─── Create reactive state ─────────────────────────
(def count (state 0))
(def name (state "helge"))
(def todos (state []))

;; ─── Read current value (@ reader macro) ───────────
@count                              ;; → 0
(string/upper @name)                ;; → "HELGE"

;; ─── Mutate — intent-based verbs ───────────────────
(put! count 42)                     ;; direct set
(update! count inc)                 ;; apply function
(update! todos conj {:text "buy milk" :done false})

;; ─── Derived / computed state ──────────────────────
(def doubled (computed (* @count 2)))
(def active (computed (remove :done @todos)))
;; auto-tracks: reads @count and @todos during computation

;; ─── Batch multiple updates (one re-render) ────────
(batch
  (put! count 0)
  (put! name "world"))

;; ─── Watch for side effects ────────────────────────
(watch count (fn [old new]
  (store/set! "count" new)))
```

**JS-side registration mapping**:

| Sema form | JS function registered | signals-core call |
|-----------|----------------------|-------------------|
| `(state val)` | `__state/create` | `signal(val)` → store in `ctx.signals`, return numeric ID |
| `@x` / `(deref x)` | `__state/deref` | `ctx.signals.get(id).value` (auto-tracked in effects) |
| `(put! x val)` | `__state/put!` | `ctx.signals.get(id).value = val` |
| `(update! x f . args)` | — | Sema fn: `(put! x (apply f (cons @x args)))` |
| `(computed expr)` | `__state/computed` | `computed(() => evalExpr())` → store, return ID |
| `(batch body...)` | `__state/batch` | `batch(() => evalBody())` |
| `(watch x fn)` | `__state/watch` | `effect()` with prev-value tracking, calls Sema fn with old+new |

**Sema-side convenience wrappers** (defined via `evalStr` at registration):
```sema
(define (state val) (__state/create val))
(define (put! ref val) (__state/put! ref val))
(define (update! ref f . args) (put! ref (apply f (cons (deref ref) args))))
(define-macro (batch . body) `(__state/batch (fn () ,@body)))
(define-macro (computed expr) `(__state/computed (fn () ,expr)))
(define (watch ref fn) (__state/watch ref fn))
```

Note: `@` reader macro is added in Phase 0.4. `deref` is registered as a native JS function. `put!` and `update!` are safe — not in `SPECIAL_FORM_NAMES`.

**Backwards compatibility**: The old Clojure names (`atom`, `reset!`, `swap!`) are NOT registered. This is a clean break — the web API uses the new vocabulary. The old names remain available in non-web Sema (CLI) where atoms are a different mechanism.

**LLM-friendliness**: The vocabulary (`state`, `put!`, `update!`, `computed`, `watch`, `batch`) maps 1:1 to concepts in every modern web framework. An `llms.txt` entry becomes: "Sema reactive state works like Solid/Vue signals. `(state val)` creates, `@x` reads, `(put! x val)` writes, `(update! x fn)` applies fn, `(computed expr)` derives, `(batch ...)` coalesces."

### 1.3 Replace innerHTML with morphdom (`component.ts` — REWRITE)

Current: `component.target.innerHTML = ""; target.appendChild(renderSip(data))` on every re-render.
New: Render sip to a detached DOM tree, then `morphdom(target, newTree)` to diff-patch.

**Mount target = component root**: The mount target element (e.g., `<div id="app">`) IS the component's root element. Component sip output is rendered as **children** of the target. This matches React's `createRoot("#app")` model — the target's tag/id are set in HTML, the component controls everything inside it. Component sip should NOT include a root wrapper element:

```sema
;; Good — content goes directly into the mount target
(defcomponent counter []
  [[:h1 "Count: " @count]               ;; fragment (list of elements)
   [:button {:on-click handle-inc} "+"]])

;; Also good — single root child
(defcomponent counter []
  [:div {:class "counter"}
    [:h1 "Count: " @count]
    [:button {:on-click handle-inc} "+"]])
```

**Rendering pattern**: Create a clone of the mount target, populate with sip children, morphdom the real target against the clone:

```ts
const clone = component.target.cloneNode(false) as Element; // shallow clone (same tag, id, etc.)
const sipNode = renderSip(sipData, interp, ctx);
clone.appendChild(sipNode);
morphdom(component.target, clone, {
  childrenOnly: true,  // only diff children, preserve target's own attributes
  onBeforeElUpdated(fromEl, toEl) {
    // Preserve focused input
    if (fromEl === document.activeElement &&
        (fromEl.tagName === 'INPUT' || fromEl.tagName === 'TEXTAREA' || fromEl.tagName === 'SELECT')) {
      for (const attr of toEl.attributes) {
        if (attr.name !== 'value') fromEl.setAttribute(attr.name, attr.value);
      }
      return false;
    }
    return true;
  },
  onNodeDiscarded(node) {
    // Clean up handles for removed DOM nodes
    if (node instanceof Element || node instanceof Text) {
      releaseHandleForNode(node, ctx);
    }
  }
});
```

Note: `childrenOnly: true` is correct here because the component's content IS the children of the mount target. The mount target's own tag/id/attributes are preserved by HTML authoring.

**Important: morphdom does NOT preserve event listeners on patched elements.** This is a known limitation (GitHub issue #29). Event delegation (1.5) is required — do NOT use direct `addEventListener` on elements inside mounted components.

**Important: morphdom focus preservation.** The `onBeforeElUpdated` callback above skips focused inputs/textareas/selects, preserving cursor position and selection. This handles the common case. For elements without IDs, morphdom may still recreate them — use stable `key` attributes on list items (via sip `{:key id}`).
```

**Fix `callComponent` race condition**: The current `callComponent` registers `__component-capture` globally on every render call. If signal updates cause cascading effects (component A's render triggers component B's render synchronously), the capture function gets stomped. Fix: use a unique per-call capture name (`__component-capture-${component.id}`) or use a closure-scoped variable with a per-instance capture mechanism.

```ts
function callComponent(interp, fnName, captureId: number): any {
  let captured: any = null;
  const captureName = `__cc_${captureId}`;
  interp.registerFunction(captureName, (val: any) => { captured = val; return null; });
  const result = interp.evalStr(`(${captureName} (${fnName}))`);
  if (result.error) { /* ... */ }
  return captured;
}
```

### 1.4 Fix handle memory leak (`handles.ts`)

Add `releaseHandle(id)` and auto-release for event handles. Event handles are short-lived — release them in a `finally` block after event handler completes.

```ts
// Auto-release event handles after handler runs
function dispatchSemaEvent(interp, ctx, callbackName, ev) {
  const evHandle = storeHandle(ev, ctx);
  try { interp.evalStr(`(${callbackName} ${evHandle})`); }
  finally { ctx.handles.delete(evHandle); }
}
```

Add `dom/release-handle!` for explicit cleanup from Sema code.

### 1.5 Event delegation for morphdom compatibility (`sip.ts`, `component.ts`)

morphdom patches remove directly-attached event listeners. Switch to event delegation.

**Design**: For each mounted component, attach delegated listeners on the mount target. Use `data-sema-on-{event}` attributes on elements. The delegator walks up from `ev.target` firing ALL matching ancestors (not just the innermost), matching standard DOM bubbling semantics:

```ts
class EventDelegator {
  setup(target: Element, interp, ctx) {
    // Bubbling events — delegated via closest() walkup
    const bubbling = [
      'click', 'dblclick', 'contextmenu',
      'input', 'change', 'submit',
      'keydown', 'keyup', 'keypress',
      'pointerdown', 'pointerup', 'pointermove',
      'focusin', 'focusout',  // use instead of focus/blur (which don't bubble)
    ];

    for (const event of bubbling) {
      target.addEventListener(event, (ev) => {
        // Walk up from target, fire ALL matching handlers (like real bubbling)
        let el = ev.target as Element | null;
        while (el && target.contains(el)) {
          const attr = `data-sema-on-${event}`;
          if (el.hasAttribute(attr)) {
            const fn = el.getAttribute(attr)!;
            if (SEMA_IDENT_RE.test(fn)) {
              dispatchSemaEvent(interp, ctx, fn, ev);
            }
          }
          el = el.parentElement;
        }
      });
    }

    // mouseenter/mouseleave — DON'T bubble. Delegate via mouseover/mouseout + relatedTarget guard
    target.addEventListener('mouseover', (ev) => {
      const el = (ev.target as Element).closest?.('[data-sema-on-mouseenter]');
      if (!el || el.contains(ev.relatedTarget as Node)) return;
      dispatchSemaEvent(interp, ctx, el.getAttribute('data-sema-on-mouseenter')!, ev);
    });
    target.addEventListener('mouseout', (ev) => {
      const el = (ev.target as Element).closest?.('[data-sema-on-mouseleave]');
      if (!el || el.contains(ev.relatedTarget as Node)) return;
      dispatchSemaEvent(interp, ctx, el.getAttribute('data-sema-on-mouseleave')!, ev);
    });
  }
}
```

**Non-bubbling events not covered by delegation**: `scroll`, `resize`, `load`, `error` (on images/iframes). These are rare in component templates. If needed, use `dom/on!` outside a mounted component, or use morphdom's `onElUpdated` to reattach. Document this limitation.

**`dom/on!` inside mounted components**: Listeners attached via `dom/on!` will be lost when morphdom patches the element. Add a runtime warning via `ctx.onerror` when `dom/on!` targets an element inside a mounted component's subtree. Recommend `on-{event}` sip attributes instead.

**SIP renderer change**: `on-click` attributes now produce `data-sema-on-click="handlerName"` instead of calling `addEventListener`. The `SEMA_IDENT_RE` validation stays.

### 1.6 Fix sync/async loader (`loader.ts`)

Change `interp.evalStr(code)` → `await interp.evalStrAsync(code)`. Also extend `SemaInterpreterLike` interface to include `evalStrAsync`. Scripts are still processed sequentially (for-of with await) to preserve execution order — script A's definitions must be available to script B.

### 1.7 Fix minor bugs

- **`dom/query-all`**: Return array of handles directly (not JSON string)
- **`dom/render`**: Expose SIP renderer — `(dom/render markup)` returns a handle ID, `(dom/render-into! selector markup)` renders into a target element. Calls `renderSip()` internally. Useful for tooltips, modals, interop with vanilla JS libraries
- **`store/get` type bug**: Always JSON-serialize values on set, JSON-parse on get. No heuristic parsing
- **Event handler errors**: Route through `ctx.onerror(error, "event:click:handlerName")` — configurable, defaults to `console.error`
- **Dead code**: Remove unused `watcherKeys` in component.ts
- **`disabled=false` bug**: In sip.ts, when `disabled` is falsy, explicitly `removeAttribute("disabled")`

### 1.8 Build system upgrade

Current: raw `tsc`. New: `tsup` for ESM + CJS + types + sourcemaps.

```json
{
  "exports": {
    ".": { "import": "./dist/index.mjs", "require": "./dist/index.cjs", "types": "./dist/index.d.ts" }
  },
  "scripts": {
    "build": "tsup src/index.ts --format esm,cjs --dts --sourcemap",
    "test": "vitest run",
    "test:e2e": "playwright test"
  },
  "dependencies": {
    "@preact/signals-core": "^1.x",
    "morphdom": "^2.x"
  },
  "peerDependencies": {
    "@sema-lang/sema": "^1.9.0"
  }
}
```

Note: `@sema-lang/sema` as peerDependency (not workspace dep) since it needs the published WASM binary.

### 1.9 Unit tests (Vitest + jsdom)

Unit tests mock `SemaInterpreterLike` — no WASM binary needed. Test every module:

- **`reactive.test.ts`**: state/deref/put!/update!, computed auto-tracking, batch coalescing, watch callbacks with old+new, deref on unknown ID throws, nested tracking isolation, watcher-throws-doesn't-break-others, update! with non-function error
- **`handles.test.ts`**: store/retrieve/release, event handle auto-cleanup, stale handle errors, `storeHandle(null)` returns null, handle ID per-context isolation, `getNode` on Event handle throws, `getEvent` on Element handle throws
- **`sip.test.ts`**: tag rendering, attributes (class, style, value, checked, disabled=false removes attr), event handler attributes (data-sema-on-*), fragments `[[":p","a"],[":p","b"]]`, nested 20+ levels, nil/empty/null children, invalid handler name logged not thrown
- **`dom.test.ts`**: query/create/append/remove/attributes/classes/styles/text/events, query-all returns array (not JSON string), event error routes to ctx.onerror, off! with unknown key is no-op, prevent-default on stale handle throws
- **`store.test.ts`**: get/set/remove/clear/keys/has for both localStorage and sessionStorage, type preservation (string "42" stays string), keys returns array (not JSON), localStorage unavailable throws SemaError
- **`component.test.ts`**: mount/unmount, reactive re-render on signal change, batch updates → single re-render, cleanup on unmount removes watchers, unmount non-existent selector is no-op, remount doesn't duplicate watchers, null-returning component clears target, callComponent race with unique capture IDs
- **`loader.test.ts`**: inline scripts, external src (mock fetch), evalStrAsync called (not evalStr), empty script skipped, 404 logged + continues, multiple scripts execute in order, custom type option
- **`context.test.ts`**: two contexts have independent handle IDs, signals, components

---

## Phase 2: Missing Features

### 2.1 Component system + lifecycle

**Design decision: NO positional hooks (no React-style call-order tracking).** React hooks require a compiler + reconciler to track call order. Sema has neither — conditional hooks would silently corrupt state. Instead, use **named local state** and **explicit lifecycle callbacks**.

```sema
;; ─── Define a component ────────────────────────────
(defcomponent timer-widget []
  ;; Local state — name-based, scoped to this component instance
  (let [count (local "count" 0)
        timer-id (local "timer-id" nil)]

    ;; Lifecycle: runs once after first render, returns cleanup fn
    (on-mount (fn []
      (let [id (js/set-interval #(update! count inc) 1000)]
        (put! timer-id id)
        ;; cleanup: called on unmount
        (fn [] (js/clear-interval @timer-id)))))

    ;; Template — @ reads auto-tracked by signals
    [:div
      [:p "Elapsed: " @count "s"]
      [:button {:on-click #(put! count 0)} "Reset"]]))

;; ─── Mount to DOM ──────────────────────────────────
(mount! "#app" timer-widget)

;; ─── Full example: todo app ────────────────────────
(def todos (state []))
(def filter-mode (state :all))

(def visible (computed
  (case @filter-mode
    :all       @todos
    :active    (remove :done @todos)
    :done      (filter :done @todos))))

(defcomponent todo-app []
  (let [input-text (local "input" "")]
    [:div
      [:input {:value @input-text
               :on-input #(put! input-text (dom/event-value %))}]
      [:button {:on-click #(batch
                             (update! todos conj {:text @input-text :done false})
                             (put! input-text ""))}
        "Add"]
      [:ul (map (fn [t] [:li {:class (when (:done t) "done")} (:text t)])
                @visible)]
      [:div
        [:button {:on-click #(put! filter-mode :all)} "All"]
        [:button {:on-click #(put! filter-mode :active)} "Active"]
        [:button {:on-click #(put! filter-mode :done)} "Done"]]]))

(mount! "#app" todo-app)
```

**`defcomponent`** is a macro that expands to a function + component metadata registration. It gives us a scope for `local` and `on-mount`:

```sema
;; (defcomponent name [props] body...) expands roughly to:
(define name
  (with-meta
    (fn [props] body...)
    {:component true}))
```

**`local` and `on-mount` require component render context.** Sema code needs to know "which component is currently rendering." The mechanism:

1. TypeScript registers `__component/current-id` as a native function that returns the top of a render context stack
2. Before calling `evalStr` to render a component, TypeScript pushes the component ID onto a JS-side stack
3. After eval returns, TypeScript pops the stack
4. `local` calls `__component/current-id` to look up its component instance, then creates/retrieves a signal from `MountedComponent.localState: Map<string, Signal>`
5. `on-mount` does the same to register its cleanup callback on the correct component

```ts
// In component.ts
const renderContextStack: string[] = [];

function callComponent(interp, fnName, captureId, ctx) {
  renderContextStack.push(fnName);
  interp.registerFunction('__component/current-id', () => renderContextStack[renderContextStack.length - 1] ?? null);
  try {
    // ... existing capture logic ...
  } finally {
    renderContextStack.pop();
  }
}
```

`local` and `on-mount` are registered as JS functions (not Sema macros) that call `__component/current-id` internally. No `EvalContext.context_stacks` modification needed — the render stack lives entirely in TypeScript.

**`local`** creates or retrieves a signal keyed by `(componentId, name)` — stored in `MountedComponent.localState: Map<string, Signal>`. First render creates; subsequent renders retrieve. No call-order dependency, no "rules of hooks."

**`on-mount`** registers a callback run once after first render. The cleanup fn returned is called on unmount. Stored in `MountedComponent.mountCleanup`.

**`mount!`** unchanged — mounts a component function to a CSS selector. Internally wraps render in a signals-core `effect()` for auto-dependency tracking.

### 2.2 `http/*` browser namespace (`http.ts` — NEW)

The WASM interpreter already handles `http/get` and `http/post` via the replay-with-cache mechanism. These work in browser context when called via `evalStrAsync`. No additional JS registration needed for basic HTTP.

Additional browser-specific wrappers registered as JS functions:
- `http/form-data` — create FormData
- `http/abort` — AbortController integration
- `http/sse` — EventSource wrapper returning a signal (for streaming)

### 2.3 `router/*` SPA routing (`router.ts` — NEW)

Hash-based router built on signals:

```sema
(router/init! {"/todos" todo-page
               "/todos/:id" todo-detail
               "/settings" settings-page})

(router/push! "/todos/42")
(router/current)  ;; → {:path "/todos/42" :params {:id "42"}}
```

Implementation: `currentRoute` is a signal in `ctx`. `hashchange` event listener updates it. `router/init!` stores route table. `router/view` component reads `currentRoute` signal and renders matching component.

### 2.4 `css/*` scoped styles (`css.ts` — NEW)

```sema
(def card-style
  (css {:background "#fff"
        :border-radius "8px"
        :padding "16px"
        :&:hover {:box-shadow "0 4px 12px rgba(0,0,0,0.15)"}}))

[:div {:class card-style} "Hello"]
```

Implementation: `css` function generates a unique class name, injects a `<style>` tag with scoped rules. Supports nesting via `&` prefix. Returns class name string.

### 2.5 Tests for new features

- **`router.test.ts`**: route matching, param extraction `:id`, navigation, signal updates, back button
- **`css.test.ts`**: class generation, style injection, nesting, pseudo-selectors, cleanup on dispose
- **`http.test.ts`**: SSE wrapper, abort controller

---

## Phase 3: LLM Proxy Hardening

### 3.1 Enforce `maxBodySize` (`handler.ts`)

Currently declared in `ProxyConfig` but never checked. Add body size check before JSON parse.

### 3.2 Rate limiting (`handler.ts`)

Sliding window rate limiter (in-memory Map, configurable per-IP or per-token):

```ts
interface RateLimitConfig {
  windowMs: number;     // default 60000
  maxRequests: number;  // default 60
}
```

### 3.3 SSE streaming support (`handler.ts`)

Add `/stream` endpoint that returns `text/event-stream`. Provider-specific streaming format parsing for OpenAI/Anthropic/Gemini.

### 3.4 Structured error codes (`types.ts`)

```ts
interface ProxyErrorResponse {
  error: string;
  code: "AUTH_FAILED" | "RATE_LIMITED" | "PROVIDER_ERROR" | "INVALID_REQUEST" | "TIMEOUT";
  details?: string;
}
```

### 3.5 Proxy tests

- Unit tests for handler (mocked fetch)
- Provider format tests (request/response serialization)
- Rate limit tests
- Auth tests

---

## Phase 4: Streaming LLM in Browser

### 4.1 `llm/stream` with reactive signal (`llm.ts`)

```sema
(def stream (llm/chat-stream messages opts))
;; stream is a signal: {:text "" :done false}
;; Updates reactively as tokens arrive
(deref stream)  ;; → {:text "Hello wo" :done false}
;; ... later
(deref stream)  ;; → {:text "Hello world!" :done true}
```

Implementation: Returns a signal ID. JS-side `EventSource` reads SSE from proxy, updates signal value on each chunk. Components using `(deref stream)` auto-re-render via effect().

### 4.2 Tests

- Mock SSE server, verify signal updates progressively
- E2E: streaming text appears in DOM incrementally

---

## Phase 5: E2E Tests (Playwright)

### Test infrastructure

```
packages/sema-web/
  e2e/
    fixtures/
      basic.html              # SemaWeb.init() + inline script
      counter.html            # imperative counter
      reactive-counter.html   # mount! + atoms
      error-recovery.html     # render error + event handler error
      multi-instance.html     # two SemaWeb instances on same page
      store-persistence.html  # store/set! + reload
      large-list.html         # 1000-item list
      focus.html              # input focus preservation
      llm-chat.html           # LLM proxy round-trip
      no-autoload.html        # autoLoad: false
      shared-atoms.html       # two components sharing atoms
    scripts/
      counter-reactive.sema
      error-component.sema
      large-list.sema
      focus-test.sema
      shared-atoms.sema
    mock-proxy.ts             # Node http server for LLM tests
    helpers.ts                # shared test utilities
    tests/
      script-loading.spec.ts
      reactive-counter.spec.ts
      focus-preservation.spec.ts
      store-persistence.spec.ts
      error-recovery.spec.ts
      multi-instance.spec.ts
      large-list.spec.ts
      memory-stability.spec.ts
      llm-roundtrip.spec.ts
      shared-atoms.spec.ts
      unmount-remount.spec.ts
      no-autoload.spec.ts
      script-load-failure.spec.ts
  playwright.config.ts
  vite.config.ts              # for serving fixtures with WASM headers
```

### Fixture design

Each fixture HTML must:
- Load WASM binary from local build (not CDN)
- Expose `window.__semaWeb` for test access
- Set `window.__semaInitialized = true` after `init()` completes

Use real WASM binary (not mocks) — the primary value of E2E is validating the full WASM↔JS↔DOM pipeline.

### Playwright config

```ts
// playwright.config.ts
export default defineConfig({
  testDir: "./e2e/tests",
  fullyParallel: true,
  use: { baseURL: "http://localhost:5173", trace: "on-first-retry" },
  webServer: [
    {
      command: "vite e2e/fixtures --port 5173",
      url: "http://localhost:5173",
      reuseExistingServer: !process.env.CI,
    },
    {
      command: "tsx e2e/mock-proxy.ts --port 3002",
      url: "http://localhost:3002/health",
      reuseExistingServer: !process.env.CI,
    },
  ],
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox",  use: { ...devices["Desktop Firefox"] } },
    { name: "webkit",   use: { ...devices["Desktop Safari"] } },
  ],
});
```

### Vite config for WASM serving

```ts
export default defineConfig({
  server: {
    headers: {
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
    },
  },
  optimizeDeps: { exclude: ["@sema-lang/sema"] },
  assetsInclude: ["**/*.wasm"],
});
```

### Mock LLM proxy

Node http server with:
- CORS preflight handling
- `/health` endpoint for Playwright readiness check
- `POST /mock-proxy/set-response` to configure canned responses per test
- Request recording for assertion (`GET /mock-proxy/requests`)
- SSE streaming support for Phase 4 tests

### Test scenarios (expanded)

**1. Script loading** (`script-loading.spec.ts`)
- Inline `<script type="text/sema">` discovered and evaluated, DOM updated
- External `src` script fetched and evaluated
- Multiple scripts execute in document order (A defines fn, B calls it)
- Custom `type="application/sema"` with `loader: { type: "application/sema" }`

**2. Reactive counter** (`reactive-counter.spec.ts`)
- Click "+", count increments in DOM
- Click "-", count decrements
- 100 rapid clicks result in count=100 (final value correct after coalescing)
- Reset button sets count to 0

**3. Focus preservation** (`focus-preservation.spec.ts`)
- Mount component with `<input>`, click input, type "hello"
- Trigger re-render via atom change (not affecting the input)
- Assert: input still has focus, value is "hello", cursor at position 5
- Assert: `document.activeElement.tagName === 'INPUT'`

**4. Store persistence** (`store-persistence.spec.ts`)
- Evaluate `(store/set! "counter" 42)`, reload page
- Assert `(store/get "counter")` returns 42
- Evaluate `(store/remove! "counter")`, reload
- Assert `(store/get "counter")` returns null
- Session storage: set value, open new tab (different session), value absent

**5. Error recovery** (`error-recovery.spec.ts`)
- Component that conditionally throws during render
- Set error-trigger atom → render throws → page doesn't crash
- Console.error called with `[sema-web] Component error`
- Clear error-trigger atom → component recovers, renders normally
- Event handler that throws → click still works for other buttons on page
- No unhandled promise rejections detected

**6. Multi-instance isolation** (`multi-instance.spec.ts`)
- Two `SemaWeb.create()` mounting to `#app-a` and `#app-b`
- Increment counter in instance A
- Assert instance B's counter unchanged
- State from instance A inaccessible from instance B

**7. Shared state between components** (`shared-atoms.spec.ts`)
- Two `mount!` calls on `#display-a` and `#display-b`, both reading `shared-count`
- `(put! shared-count 42)`
- Both display elements show 42

**8. Component unmount + remount** (`unmount-remount.spec.ts`)
- Mount component, verify renders
- Unmount, target is empty
- Remount same component on same selector, renders correctly
- After remount, state changes trigger re-renders (no stale/duplicate watchers)
- After 10 `put!` calls, no more than 10 re-renders (no watcher accumulation)

**9. Large DOM tree** (`large-list.spec.ts`)
- Component renders 1000 `<li>` elements
- Assert all 1000 are in DOM
- Update single item, assert morphdom patches only changed node (use MutationObserver count)
- Re-render completes within 200ms (bounded, not 16ms — large trees are legitimately slower)

**10. Memory stability** (`memory-stability.spec.ts`)
- Force 500 re-renders of a component with event handlers
- Assert handle map size doesn't grow unboundedly (event handles auto-released)
- Assert no uncollected DOM nodes via WeakRef sentinel
- Chromium-only: `performance.memory.usedJSHeapSize` stays bounded

**11. LLM round-trip** (`llm-roundtrip.spec.ts`)
- Configure mock proxy with canned response
- Type message, click send
- Loading indicator appears
- "Mock response" appears in messages
- Loading indicator disappears
- Mock proxy received correct JSON body and Authorization header

**12. Script load failure** (`script-load-failure.spec.ts`)
- External script pointing to 404 URL
- `SemaWeb.init()` still resolves (doesn't reject)
- Console.error logged with URL
- Subsequent inline script still executes

**13. No autoLoad** (`no-autoload.spec.ts`)
- `SemaWeb.create({ autoLoad: false })` with scripts present
- Scripts not evaluated, DOM unchanged

**14. Router navigation** (Phase 2, `router-navigation.spec.ts`)
- Init routes, navigate via `router/push!`
- URL hash changes, correct component renders
- Back button returns to previous route
- Direct URL entry `#/todos/42` renders correct component with params

**15. Streaming LLM** (Phase 4, `llm-streaming.spec.ts`)
- Mock SSE proxy sends tokens incrementally
- Signal value updates progressively
- DOM shows text growing character-by-character
- "done" flag set when stream completes

### Shared test helpers (`e2e/helpers.ts`)

```ts
export async function waitForSema(page: Page): Promise<void> {
  await page.waitForFunction(() => (window as any).__semaInitialized === true, null, { timeout: 10000 });
}

export async function semaEval(page: Page, code: string): Promise<any> {
  return page.evaluate((code) => (window as any).__semaWeb.eval(code), code);
}

export function captureConsoleErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", (msg) => { if (msg.type() === "error") errors.push(msg.text()); });
  return errors;
}
```

### Browser compatibility

- **Chromium**: all tests
- **Firefox**: all tests except `performance.memory` (Chromium-only)
- **WebKit**: all tests, watch for stricter CORS and `localStorage` quota behavior
- CI runs Chromium only (fast). Firefox + WebKit weekly or on release branches.

---

## Integration Tests (Vitest, with real WASM)

Between unit tests (mock interpreter) and E2E (Playwright), add integration tests that run in Vitest with the real WASM module loaded:

- **`integration/register-function.test.ts`**: Register JS function, call from Sema, verify round-trip
- **`integration/async-replay.test.ts`**: `evalStrAsync` with `http/get` mock, verify replay mechanism works
- **`integration/multi-instance.test.ts`**: Two `SemaWeb` instances, verify complete state isolation
- **`integration/dispose.test.ts`**: After `dispose()`, `eval()` throws descriptive error (not WASM panic)
- **`integration/preload-module.test.ts`**: `preloadModule` + `(import ...)` works end-to-end

These require the WASM binary and run slower — separate vitest project or `--project integration` flag.

---

## CI Integration

### Dependency chain

```
Rust build → sema-wasm .wasm → @sema-lang/sema package → @sema-lang/sema-web → tests
```

### GitHub Actions workflow

```yaml
# .github/workflows/sema-web-tests.yml
name: sema-web tests
on:
  push:
    paths: [packages/sema-web/**, crates/sema-wasm/**, packages/sema/**]

jobs:
  unit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - run: npm ci --workspace=packages/sema-web
      - run: npm test --workspace=packages/sema-web

  e2e:
    runs-on: ubuntu-latest
    needs: unit
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: wasm32-unknown-unknown }
      - uses: actions/cache@v4
        id: wasm-cache
        with:
          path: packages/sema-wasm/pkg
          key: wasm-${{ hashFiles('crates/sema-wasm/src/**', 'Cargo.lock') }}
      - if: steps.wasm-cache.outputs.cache-hit != 'true'
        run: cargo install wasm-pack && wasm-pack build crates/sema-wasm --target web
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - run: npm ci
      - run: npm run build --workspace=packages/sema-web
      - run: npx playwright install --with-deps chromium
      - run: npm run test:e2e --workspace=packages/sema-web
      - uses: actions/upload-artifact@v4
        if: failure()
        with: { name: playwright-report, path: packages/sema-web/playwright-report/ }
```

---

## File Change Summary

### Modified files
| File | Changes |
|------|---------|
| `packages/sema-web/src/index.ts` | Create SemaWebContext, pass to all register fns |
| `packages/sema-web/src/reactive.ts` | Full rewrite: @preact/signals-core |
| `packages/sema-web/src/component.ts` | Full rewrite: effect() + morphdom + unique capture IDs |
| `packages/sema-web/src/sip.ts` | Event delegation via data-sema-on-*, ctx param, disabled=false fix |
| `packages/sema-web/src/handles.ts` | Instance-scoped via ctx, releaseHandle, event auto-cleanup |
| `packages/sema-web/src/dom.ts` | ctx param, fix query-all, errors → ctx.onerror |
| `packages/sema-web/src/store.ts` | ctx param, always JSON-serialize values |
| `packages/sema-web/src/loader.ts` | evalStr → evalStrAsync, extend interface |
| `packages/sema-web/src/llm.ts` | Streaming support, error propagation |
| `packages/sema-web/package.json` | Add deps, scripts, build config, peerDeps |
| `packages/llm-proxy/src/handler.ts` | maxBodySize enforcement, rate limiting, streaming |
| `packages/llm-proxy/src/types.ts` | Error codes, rate limit config |

### New files
| File | Purpose |
|------|---------|
| Root `package.json` | npm workspaces config |
| `packages/sema-wasm/package.json` | Correct scoped name for local WASM pkg |
| `packages/sema-web/src/context.ts` | SemaWebContext class |
| `packages/sema-web/src/router.ts` | Hash-based SPA router |
| `packages/sema-web/src/css.ts` | Scoped CSS injection |
| `packages/sema-web/src/http.ts` | Browser HTTP wrappers (SSE, abort) |
| `packages/sema-web/tsup.config.ts` | Build configuration |
| `packages/sema-web/vitest.config.ts` | Unit + integration test config |
| `packages/sema-web/playwright.config.ts` | E2E test config |
| `packages/sema-web/e2e/vite.config.ts` | WASM serving headers |
| `packages/sema-web/e2e/mock-proxy.ts` | LLM test mock server |
| `packages/sema-web/e2e/helpers.ts` | Shared Playwright helpers |
| `packages/sema-web/tests/*.test.ts` | Unit tests (one per module) |
| `packages/sema-web/tests/integration/*.test.ts` | WASM integration tests |
| `packages/sema-web/e2e/fixtures/*.html` | E2E test pages |
| `packages/sema-web/e2e/fixtures/scripts/*.sema` | E2E test Sema scripts |
| `packages/sema-web/e2e/tests/*.spec.ts` | E2E test specs |
| `.github/workflows/sema-web-tests.yml` | CI workflow |

### No changes needed
- `crates/sema-wasm/` — no Rust changes
- `packages/sema/` — JS wrapper unchanged
- `packages/llm-proxy/src/providers.ts` — provider specs unchanged
- `packages/llm-proxy/src/adapters/` — platform adapters unchanged

---

## Verification

### Phase 0 verification
```bash
npm install          # root workspace installs all packages
npm run build:wasm   # builds sema-wasm (skip if using published pkg)
```

### Phase 1 verification
```bash
cd packages/sema-web
npm run build           # tsup builds ESM+CJS+types
npm test                # vitest: all unit tests pass
```

Manual check: Open `examples/web/index.html` in browser, verify counter works with reactive updates and morphdom (focus preserved in inputs).

### Phase 2-4 verification
```bash
npm test                # all unit tests including new modules
npm run test:e2e        # playwright: all E2E scenarios pass
```

### Full verification
```bash
# From repo root
npm run build
npm test
cd packages/sema-web && npm run test:e2e
```

---

## Execution Orchestration

### Step 1: Phase 0 — Infrastructure (sequential, ~1 session)

All subsequent work depends on this. Cannot parallelize.

**1a. Root workspace + WASM package** (single agent)
- Create root `package.json` with workspaces
- Create `packages/sema-wasm/package.json` with specified content
- Fix wasm-pack output directory
- Verify: `npm install` from root succeeds, all packages resolve

**1b. `@` reader macro** (single agent, Rust change)
- Add `Token::Deref` to lexer, parser dispatch in `crates/sema-reader/src/reader.rs`
- Add tests: `@x` → `(deref x)`, `@(+ 1 2)` → `(deref (+ 1 2))`
- Remove `assert!(read("@").is_err())` test
- Verify: `make test` — all Rust tests pass including new `@` tests

**1c. Verify `put!`/`update!` are safe names** (quick grep, no agent needed)

**Checkpoint**: `make test` passes, `npm install` works from root.

---

### Step 2: Phase 1 foundation (parallel agents where possible)

Phase 1 is the largest phase. Split into independent work streams:

**Stream A: Core refactoring** (1 agent, sequential — everything else depends on this)
- 1.1 Create `context.ts` (SemaWebContext class)
- 1.2 Rewrite `reactive.ts` with @preact/signals-core
- Rename `hiccup.ts` → `sip.ts`, update all imports/exports
- Update `index.ts` to create context, pass to all register fns
- Add `publishConfig`, `sideEffects`, update package.json deps
- Verify: `npm run build` succeeds (tsup)

**Stream B: Build system + test infrastructure** (1 agent, can run in parallel with Stream A on a worktree)
- 1.8 Add tsup config, vitest config, playwright config
- Add `@preact/signals-core` and `morphdom` to package.json
- Create Vite config for E2E fixtures with WASM headers
- Create mock LLM proxy server skeleton
- Create E2E helpers.ts
- Verify: `npm run build` produces dist/, `vitest --version` works, `playwright --version` works

**After A+B merge:**

**Stream C: morphdom + event delegation + fixes** (1 agent)
- 1.3 Rewrite `component.ts` — morphdom integration, effect()-based rendering, unique capture IDs, error boundaries
- 1.4 Fix handle leak in `handles.ts` — releaseHandle, event auto-cleanup, onNodeDiscarded
- 1.5 Event delegation in `sip.ts` — EventDelegator class, bubbling walkup, mouseenter/mouseleave via mouseover/mouseout
- 1.7 Fix minor bugs: dom/query-all, store/get type bug, dom/render + dom/render-into!, dom/event-value, disabled=false, error routing
- 1.6 Fix loader.ts — evalStrAsync
- Register missing helpers: js/set-interval, js/clear-interval
- Verify: `npm run build` succeeds, counter example works in browser

**Stream D: Unit tests** (1 agent, can start after Stream A, parallel with Stream C)
- 1.9 Write all unit tests: reactive.test.ts, handles.test.ts, sip.test.ts, dom.test.ts, store.test.ts, component.test.ts, loader.test.ts, context.test.ts
- Verify: `npm test` — all unit tests pass

**Checkpoint**: `npm run build && npm test` passes. Counter example works in browser with reactive updates and focus preservation.

---

### Step 3: Phases 2, 3, 4 (parallel agents)

All three are independent after Phase 1. Run simultaneously:

**Agent 1: Phase 2 — Features** (worktree)
- 2.1 Component system: defcomponent macro, local, on-mount, renderContextStack on ctx
- 2.2 http.ts — browser HTTP wrappers (SSE, abort)
- 2.3 router.ts — hash-based SPA router
- 2.4 css.ts — scoped CSS injection
- 2.5 Unit tests for all new modules
- Verify: `npm test` — all new + existing tests pass

**Agent 2: Phase 3 — LLM proxy hardening** (worktree)
- 3.1 maxBodySize enforcement
- 3.2 Rate limiting
- 3.3 SSE streaming support
- 3.4 Structured error codes
- 3.5 Proxy unit tests
- Verify: `cd packages/llm-proxy && npm test`

**Agent 3: Phase 4 — Streaming LLM in browser** (worktree)
- 4.1 llm/chat-stream returning reactive signal
- 4.2 Unit tests + mock SSE tests
- Verify: `npm test` — streaming tests pass

**Checkpoint**: All three worktrees merge cleanly. `npm run build && npm test` passes for all packages.

---

### Step 4: Phase 5 — E2E tests (sequential after Step 3)

Depends on all features being implemented. Single agent:

- Create all fixture HTML pages (basic, counter, reactive-counter, error-recovery, multi-instance, store, focus, large-list, shared-atoms, llm-chat, no-autoload)
- Create all .sema test scripts
- Write all 15 E2E test specs
- Configure Playwright projects (chromium, firefox, webkit)
- Verify: `npm run test:e2e` — all specs pass on chromium

---

### Step 5: Integration tests + CI (parallel agents)

**Agent 1: Integration tests**
- Write WASM integration tests (register-function, async-replay, multi-instance, dispose, preload-module)
- Verify: `npm test` — integration suite passes

**Agent 2: CI workflow**
- Create `.github/workflows/sema-web-tests.yml`
- WASM build caching, unit → E2E dependency chain
- Verify: workflow file is valid YAML, references correct paths

**Checkpoint**: Full test suite passes: `npm run build && npm test && cd packages/sema-web && npm run test:e2e`

---

### Step 6: Phase 6 — Documentation (parallel agents)

All docs are independent pages. Launch up to 3 agents in parallel:

**Agent 1**: index.md, getting-started.md, reactive-state.md, components.md
**Agent 2**: sip-markup.md, dom-api.md, store.md, routing.md, css.md
**Agent 3**: llm.md, llm-proxy.md, deployment.md, examples.md, llms.txt, sidebar config

**Checkpoint**: `cd website && npm run dev` — all doc pages render, sidebar navigation works.

---

### Step 7: Final (sequential)

- Update CHANGELOG.md with all changes
- Bump versions to 2.0.0 for sema-web, minor bump for llm-proxy
- Full verification: `make test && npm run build && npm test && cd packages/sema-web && npm run test:e2e`
- Code review via superpowers:code-reviewer agent

---

### Summary: Agent Parallelization Map

```
Step 1:  [Phase 0: infra]──[Phase 0: @ macro]
              │
Step 2:  [Stream A: core]═══[Stream B: build/test infra]  (parallel, worktree)
              │                        │
              └────────┬───────────────┘
                       │
         [Stream C: morphdom+fixes]═══[Stream D: unit tests]  (parallel)
              │                        │
              └────────┬───────────────┘
                       │
Step 3:  [Phase 2: features]═══[Phase 3: proxy]═══[Phase 4: streaming]  (3 parallel worktrees)
              │                      │                    │
              └──────────┬───────────┴────────────────────┘
                         │
Step 4:  [Phase 5: E2E tests]
                         │
Step 5:  [Integration tests]═══[CI workflow]  (parallel)
                         │            │
                         └─────┬──────┘
                               │
Step 6:  [Docs agent 1]═══[Docs agent 2]═══[Docs agent 3]  (parallel)
                               │
Step 7:  [CHANGELOG + version bump + final verification]
```

`═══` = parallel, `───` = sequential dependency

---

## Implementation Notes (cross-cutting concerns)

### File renames
- `packages/sema-web/src/hiccup.ts` → rename to `sip.ts`. Delete the old file. Update all imports in `index.ts` (`registerHiccupBindings` → `registerSipBindings`, `renderHiccup` → `renderSip`). Update `SemaWebOptions.hiccup` → `SemaWebOptions.sip`.

### Missing helper functions
- **`js/set-interval`, `js/clear-interval`**: Register as JS native functions in `dom.ts` (or a new `timer.ts`). Wrap browser `setInterval`/`clearInterval`. Used by component lifecycle examples.
- **`dom/event-value`**: Register in `dom.ts` — reads `event.target.value` from an event handle. Essential for form inputs.
- **`defcomponent` macro**: Define via `evalStr` at registration time (Sema macro, not Rust). Expands to `(define name (with-meta (fn [props] body...) {:component true}))`. Works in both tree-walker and VM since it's a user-space macro.

### `renderContextStack` must be per-instance
Move from module-level `const` to a field on `SemaWebContext` to maintain multi-instance isolation. Same rationale as Phase 1.1 for all other module-level state.

### `deref` registration
Phase 0.4 adds the `@` reader macro (syntax only — `@x` → `(deref x)`). Phase 0.4 does NOT register `deref` as a function. Phase 1.2 registers `__state/deref` as the JS-side native and defines `(define (deref x) (__state/deref x))` via `evalStr`. Single registration point, no conflict.

### Error boundaries
When a component render function throws, `callComponent` catches the error, routes it to `ctx.onerror`, and leaves the mount target showing its last successfully rendered content (no blank flash). The component's `effect()` remains active so the next state change retries the render.

### Scope exclusions
- **SSR and hydration** are explicitly out of scope for this iteration. Sema Web is client-side only.
- **AOT compilation** (`sema build --target wasm`) is Phase 4 of the original issue, deferred to a future plan.

### Versioning
These changes are breaking (old `atom`/`reset!`/`swap!` API removed, `hiccup` → `sip` rename). Bump to `2.0.0` for `@sema-lang/sema-web`. `@sema-lang/llm-proxy` gets a minor bump if only hardening changes land. Update `CHANGELOG.md` with all changes per the release procedure in CLAUDE.md.

### npm publish
Add `"publishConfig": { "access": "public" }` to all `@sema-lang/*` package.json files (scoped packages default to private on npm).

### Bundle size
Target: `@sema-lang/sema-web` < 10kB gzipped (excluding the WASM binary). Add `"sideEffects": false` to `package.json` for tree-shaking. Current deps: signals-core 1.6kB + morphdom 3kB + our code ~3-5kB.

### `packages/sema-wasm/package.json` content
```json
{
  "name": "@sema-lang/sema-wasm",
  "version": "2.0.0",
  "type": "module",
  "main": "./pkg/sema_wasm.js",
  "types": "./pkg/sema_wasm.d.ts",
  "files": ["pkg/"],
  "publishConfig": { "access": "public" }
}
```

### CI wasm-pack `--out-dir`
The CI workflow must use `--out-dir ../../packages/sema-wasm/pkg` to match the root `build:wasm` script. Update the CI step to:
```yaml
wasm-pack build crates/sema-wasm --target web --out-dir ../../packages/sema-wasm/pkg
```

### Macro registration ordering
`batch` and `computed` macros use quasiquote (`\``) and unquote-splice (`,@`). These are available in Sema from the reader — quasiquote is not a prelude feature, it's a reader macro. Safe to use in `evalStr` at registration time regardless of prelude load order.

---

## Phase 6: Documentation (Website)

Add a new top-level **Sema Web** section to the website (`website/docs/web/`), separate from the existing stdlib/language/internals sections which are already long. Update the VitePress sidebar in `website/.vitepress/config.ts` to add the section.

### Site structure

```
website/docs/web/
  index.md              # Overview: what is Sema Web, quick start, architecture diagram
  getting-started.md    # Install, first page, <script type="text/sema">, SemaWeb.init()
  reactive-state.md     # state, @, put!, update!, computed, batch, watch — full API + examples
  components.md         # defcomponent, mount!, local, on-mount, lifecycle, re-rendering
  sip-markup.md         # SIP format: tags, attributes, events, style, fragments, lists with keys
  dom-api.md            # dom/* namespace: query, create, append, attributes, classes, events, render
  store.md              # store/* namespace: localStorage, sessionStorage, persistence
  routing.md            # router/* namespace: init!, push!, current, route matching
  css.md                # css/* namespace: scoped styles, nesting, pseudo-selectors
  llm.md                # llm/* namespace: complete, chat, stream, extract, classify — proxy setup
  llm-proxy.md          # @sema-lang/llm-proxy: adapters (Vercel, Netlify, Cloudflare, Node), config, auth
  deployment.md         # Deploy to Vercel/Netlify/Cloudflare: static files + LLM proxy adapter
  examples.md           # Full examples: counter, todo app, kanban, AI chat — with live playground links
```

### Sidebar addition (`website/.vitepress/config.ts`)

```ts
{
  text: 'Sema Web',
  items: [
    { text: 'Overview', link: '/docs/web/' },
    { text: 'Getting Started', link: '/docs/web/getting-started' },
    { text: 'Reactive State', link: '/docs/web/reactive-state' },
    { text: 'Components', link: '/docs/web/components' },
    { text: 'SIP Markup', link: '/docs/web/sip-markup' },
    { text: 'DOM API', link: '/docs/web/dom-api' },
    { text: 'Store', link: '/docs/web/store' },
    { text: 'Routing', link: '/docs/web/routing' },
    { text: 'Scoped CSS', link: '/docs/web/css' },
    { text: 'LLM Integration', link: '/docs/web/llm' },
    { text: 'LLM Proxy', link: '/docs/web/llm-proxy' },
    { text: 'Deployment', link: '/docs/web/deployment' },
    { text: 'Examples', link: '/docs/web/examples' },
  ]
}
```

### Page content guidelines

Each page should include:
- **API reference** — every function with signature, params, return value, and a short example
- **Conceptual explanation** — why it works this way, mental model
- **Common patterns** — idiomatic usage, not just API listing
- **Gotchas** — things that surprise people (e.g., `dom/on!` inside components loses listeners)

### Key pages in detail

**`reactive-state.md`** — The most important page. Must clearly explain:
- `(state val)` creates, `@x` reads, `(put! x val)` writes, `(update! x fn)` applies
- Auto-tracking: how `@x` inside a component or `computed` auto-subscribes
- `batch` for coalescing — when and why to use it
- `watch` for side effects — not for rendering (that's automatic)
- Comparison table: Sema vs React vs Vue vs Solid (one-liner per concept)

**`sip-markup.md`** — Explain the SIP format with a visual comparison to HTML:
```
HTML:                          SIP:
<div class="card">             [:div {:class "card"}
  <h1>Title</h1>                [:h1 "Title"]
  <p>Content</p>                [:p "Content"]]
</div>
```
Cover: tags, attributes, event handlers (`on-click`), style (string and map), fragments, conditional rendering, list rendering with keys.

**`llm-proxy.md`** — Separate from `llm.md` because it's a server-side package. Cover: why a proxy is needed (API keys), adapter installation per platform, config options, auth, rate limiting, streaming.

### `llms.txt` entry

Add to `website/public/llms.txt` (or create it):

```
# Sema Web

Sema is a Lisp that runs in the browser via WASM. Web apps use reactive state and SIP markup.

## Reactive State
- `(state val)` — create reactive state, returns reference
- `@x` — read current value (auto-tracked in components/computed)
- `(put! x val)` — set value
- `(update! x fn . args)` — apply function to current value
- `(computed expr)` — derived state, auto-tracks dependencies
- `(batch body...)` — coalesce multiple updates into one re-render
- `(watch x fn)` — side effect on change, fn receives (old new)

## SIP Markup (Sema Interface Primitives)
Components return vectors describing DOM:
- `[:tag {attrs} children...]` — element
- `{:on-click handler-name}` — event handler
- `{:class "name" :style "css"}` — attributes
- Fragments: return a list of elements

## Components
- `(defcomponent name [props] body...)` — define component
- `(mount! "#selector" component)` — mount to DOM
- `(local "name" initial)` — component-scoped state
- `(on-mount fn)` — lifecycle, return cleanup fn

## DOM
- `(dom/query sel)`, `(dom/create-element tag)`, `(dom/render markup)`
- Full DOM API under dom/* namespace

## LLM
- `(llm/complete prompt)`, `(llm/chat messages)`, `(llm/stream ...)`
- Requires proxy server — see @sema-lang/llm-proxy
```

### Documentation files

| File | Purpose |
|------|---------|
| `website/docs/web/index.md` | Overview + architecture |
| `website/docs/web/getting-started.md` | Quick start guide |
| `website/docs/web/reactive-state.md` | State management API |
| `website/docs/web/components.md` | Component system |
| `website/docs/web/sip-markup.md` | SIP format reference |
| `website/docs/web/dom-api.md` | DOM namespace reference |
| `website/docs/web/store.md` | Storage API |
| `website/docs/web/routing.md` | Router API |
| `website/docs/web/css.md` | Scoped CSS API |
| `website/docs/web/llm.md` | Browser LLM API |
| `website/docs/web/llm-proxy.md` | Server proxy package |
| `website/docs/web/deployment.md` | Deploy guide |
| `website/docs/web/examples.md` | Full app examples |
| `website/.vitepress/config.ts` | Add sidebar section |
| `website/public/llms.txt` | LLM-readable API summary |

### Execution

Documentation is written AFTER implementation is complete and tested (Phases 1-5). Can be parallelized: one agent per doc page, all referencing the implemented code.
