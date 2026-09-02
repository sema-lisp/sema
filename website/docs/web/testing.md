# Testing Components

Sema Web components are Sema functions that produce SIP markup, mounted into the
DOM by a runtime that owns their events, effects, and cleanup. Testing one by
hand therefore means booting an interpreter, registering bindings, building a
container, dispatching events that look like a browser's, and unwinding it all
afterwards — roughly forty lines of setup before the first assertion.

`@sema-lang/sema-web/testing` is that setup, done once.

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

## Requirements

- A **Node test runner** with the **`jsdom`** environment. In Vitest:
  `test: { environment: "jsdom" }`.
- The WASM interpreter, which comes with `@sema-lang/sema-web` through
  `@sema-lang/sema-wasm`. The helper reads it off disk; point it elsewhere with
  the `SEMA_WASM_PATH` environment variable or the `wasmUrl` option.

Gate a suite so a checkout without a WASM build skips instead of failing:

```js
import { semaWasmAvailable } from "@sema-lang/sema-web/testing";

const describeWithSema = semaWasmAvailable() ? describe : describe.skip;
```

The testing utilities are a **separate entry point**. Nothing in the runtime
imports them, so the Node built-ins they use cannot reach a browser bundle.

## It is a real interpreter

`renderSema` calls the same `SemaWeb.create()` your app does. Macros expand,
props marshal through the WASM value boundary, SIP renders through the real
`morphdom`, delegated events walk the real DOM, and teardown runs the real
cleanup. A component that passes here is exercising the code path a browser
runs — which is the point: a mocked interpreter cannot tell you that
`(defcomponent view (props) ...)` has the wrong arity, or that a prop key kept
its colon and now reads as nil.

What it is *not* is a browser. JSDOM has no layout, no real focus semantics
beyond the basics, and no network. Keep a Playwright test for anything that
depends on those.

## Options

```js
const screen = await renderSema(source, {
  mount: "view",              // component to mount; omit to only evaluate source
  props: { title: "Hello" },  // ":title" keys are accepted too
  target: "#app",             // mount selector
  html: '<div id="app"></div>',
  url: "#/todos/42",          // starting URL; resets to "/" when omitted
  web: { dev: false },        // merged over the harness defaults
  onerror(error, context) {}, // also forward captured errors here
  wasmUrl: "/path/to/sema_wasm_bg.wasm",
});
```

`source` is evaluated **once, before anything mounts** — put your state,
component definitions, and event handlers there. A syntax or evaluation failure
raises immediately, with the interpreter's own message, rather than handing back
an empty screen.

Omitting `props` mounts with *no* props, which is not the same as `{}`: a
component declared without parameters throws if it is called with an argument.

`url` names the route a screen boots on — `"#/todos/42"` for the default hash
mode, `"/todos/42?tab=open"` for history mode. Every screen resets the location
(to `/` when `url` is omitted), because JSDOM shares one `window` across a test
file: without that, a screen that navigated would decide which route the *next*
screen mounts on, and reordering a router test would change what it asserts.

## Reading the DOM

Every query is scoped to the mount container, so fixture markup around the
component cannot satisfy an assertion by accident.

```js
screen.html();            // innerHTML of the container
screen.html("ul");        // innerHTML of the first match
screen.text("button");    // textContent of the first match
screen.find("li");        // first match, raising with the current markup if absent
screen.query("li");       // first match or null
screen.findAll("li");     // every match, as an array
screen.container;         // the element itself
```

`find` failing prints what the container actually holds, which is almost always
the information you need.

## Driving it

```js
screen.click("button");             // runs activation behaviour, as a browser does
screen.fill("input", "hello");      // sets the value and fires `input`
screen.select("select", "b");       // sets the value and fires `change`
screen.check("input");              // clicks a checkbox only if it is not already checked
screen.check("input", false);       // ...or unchecks it
screen.mount("todo-list", {});      // mount another component into the container
screen.submit();                    // fires `submit` on the form (default selector "form")
screen.press("input", "Enter");     // fires `keydown` carrying the key
screen.fire("div", "focusin");      // any bubbling, cancelable event
screen.focus("#field");             // moves the active element
```

Events are constructed with the right class for their name — a `click` is a
`MouseEvent`, a `keydown` a `KeyboardEvent` — so a handler reading `key` or
`relatedTarget` sees what it would in a browser.

## Talking to Sema

```js
screen.run("(+ 1 2)");        // "3" — throws the interpreter's message on failure
screen.eval("(nope)").error;  // the raw result; never throws
screen.output;                // everything printed, in order, across every call
screen.signal("count");       // the JS value of a reactive global
screen.setSignal("count", 9); // write one, triggering a re-render
```

`run` is the one to reach for. `eval` exists for when the failure *is* the thing
under test.

## Asserting on failures

The runtime catches failures rather than propagating them, so a broken component
renders nothing instead of throwing into your test. `renderSema` collects
everything the runtime caught:

```js
const screen = await renderSema('(defcomponent view () (error "boom"))', { mount: "view" });

expect(screen.errors).toHaveLength(1);
expect(screen.errors[0].message).toContain("boom");
expect(screen.errorContexts()).toEqual(["component:view"]);
```

Each entry is `{ error, context, message }`, with `context` in the runtime's own
vocabulary (`component:view`, `event:click:save`, `effect:view#0`,
`resource:user`). This replaces the runtime's `console.error` default, so a suite
that exercises failure paths does not bury its own output.

Dev mode is **on** by default, which means `screen.diagnostics` carries a render
timeline and dev-only checks (duplicate SIP keys) report. Pass
`{ web: { dev: false } }` for a production-shaped run.

## Proving nothing leaked

A component owns signals, watches, intervals, streams, listeners, and lifecycle
slots. `leaks()` reports the registries that grew and nothing else, so `{}` means
clean and a failure names what is still live:

```js
screen.unmount();
expect(screen.leaks()).toEqual({});   // → { intervals: 1 } when a timer survived
```

The baseline is taken **after** `source` was evaluated and **before** anything
was mounted. Module-level state a fixture defines is the app's, not the
component's, and does not read as a leak.

Call it after `unmount()` — that is what proves component teardown. After
`dispose()` the whole context is force-drained, so a clean report there says
much less. `snapshot()` gives the raw counts if you want to diff them yourself.

## Async

```js
await screen.flush();   // lets queued microtasks and timers run
```

Use it after anything that reaches the network — `resource`, `http/*`, SSE — with
`fetch` stubbed:

```js
vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response("{}"))));

const screen = await renderSema('(def user (resource "user" (fn () "/api/user")))');
expect(screen.run("(:loading @user)")).toBe("#t");

await screen.flush();
expect(screen.run("(:loading @user)")).toBe("#f");
```

Stub `fetch` **before** `renderSema`: a resource defined in `source` starts
fetching as soon as it is created.

## Cleanup

Each screen owns a WASM instance, so dispose it:

```js
afterEach(() => disposeAllScreens());
```

`disposeAllScreens()` tears down every screen `renderSema` created that is still
live, which makes one forgotten `dispose()` harmless. `screen.dispose()` is
idempotent if you would rather be explicit.

## See also

- [Components](/docs/web/components) — props, lifecycle, effects, and cleanup rules
- [Diagnostics & Dev Mode](/docs/web/diagnostics) — the `onerror` and `dev` surfaces this builds on
- [Reactive State](/docs/web/reactive-state) — what `signal` and `setSignal` read and write
