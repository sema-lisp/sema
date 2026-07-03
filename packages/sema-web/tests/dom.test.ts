import { describe, it, expect, vi, beforeEach } from "vitest";
import { registerDomBindings } from "../src/dom.js";
import { SemaWebContext } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";
import { storeHandle } from "../src/handles.js";

describe("registerDomBindings", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    registerDomBindings(interp, ctx);
    document.body.innerHTML = "";
  });

  // --- Registration ---

  it("registers all expected dom/* functions", () => {
    const expected = [
      "dom/query", "dom/query-all", "dom/get-id",
      "dom/create-element", "dom/create-text",
      "dom/append-child!", "dom/remove-child!", "dom/remove!",
      "dom/set-attribute!", "dom/get-attribute", "dom/remove-attribute!",
      "dom/add-class!", "dom/remove-class!", "dom/toggle-class!", "dom/has-class?",
      "dom/set-style!", "dom/get-style",
      "dom/set-text!", "dom/get-text",
      "dom/set-html!", "dom/get-html",
      "dom/set-value!", "dom/get-value",
      "dom/on!", "dom/off!", "dom/prevent-default!",
      "dom/stop-propagation!", "dom/event-value", "dom/event-key",
      "dom/event-target", "dom/event-target-closest", "dom/focus!",
      "dom/render", "dom/render-into!",
    ];
    for (const name of expected) {
      expect(interp.getFunction(name), `${name} should be registered`).toBeDefined();
    }
  });

  // --- create-element ---

  it("dom/create-element returns a numeric handle", () => {
    const fn = interp.getFunction("dom/create-element")!;
    const handle = fn("div");
    expect(typeof handle).toBe("number");
  });

  it("dom/query and dom/get-id return null for missing elements without allocating handles", () => {
    expect(interp.getFunction("dom/query")!("#missing")).toBeNull();
    expect(interp.getFunction("dom/get-id")!("missing")).toBeNull();
    expect(ctx.handles.size).toBe(0);
  });

  it("dom/create-text can be appended, read, removed, and released with its parent subtree", () => {
    const parent = interp.getFunction("dom/create-element")!("div");
    const child = interp.getFunction("dom/create-text")!("hello");

    expect(interp.getFunction("dom/append-child!")!(parent, child)).toBe(child);
    expect(ctx.handles.get(parent)?.textContent).toBe("hello");

    expect(interp.getFunction("dom/remove-child!")!(parent, child)).toBe(child);
    expect(ctx.handles.get(parent)?.textContent).toBe("");
  });

  // --- set-text / get-text ---

  it("dom/set-text! and dom/get-text round-trip", () => {
    const handle = interp.getFunction("dom/create-element")!("div");
    interp.getFunction("dom/set-text!")!(handle, "hello");
    const text = interp.getFunction("dom/get-text")!(handle);
    expect(text).toBe("hello");
  });

  // --- query-all returns array ---

  it("dom/query-all returns an array", () => {
    document.body.innerHTML = "<p>a</p><p>b</p>";
    const result = interp.getFunction("dom/query-all")!("p");
    expect(Array.isArray(result)).toBe(true);
    expect(result.length).toBe(2);
  });

  // --- set-attribute / get-attribute ---

  it("dom/set-attribute! and dom/get-attribute round-trip", () => {
    const handle = interp.getFunction("dom/create-element")!("div");
    interp.getFunction("dom/set-attribute!")!(handle, "class", "foo");
    const val = interp.getFunction("dom/get-attribute")!(handle, "class");
    expect(val).toBe("foo");
  });

  // --- add-class / has-class ---

  it("dom/add-class! and dom/has-class? work", () => {
    const handle = interp.getFunction("dom/create-element")!("div");
    interp.getFunction("dom/add-class!")!(handle, "bar");
    const has = interp.getFunction("dom/has-class?")!(handle, "bar");
    expect(has).toBe(true);
  });

  it("class, style, html, and form helpers handle stateful DOM properties", () => {
    const div = interp.getFunction("dom/create-element")!("div");
    interp.getFunction("dom/add-class!")!(div, "a", "b");
    interp.getFunction("dom/remove-class!")!(div, "a");
    expect(interp.getFunction("dom/has-class?")!(div, "a")).toBe(false);
    expect(interp.getFunction("dom/toggle-class!")!(div, "open")).toBe(true);

    interp.getFunction("dom/set-style!")!(div, "background-color", "red");
    expect(interp.getFunction("dom/get-style")!(div, "background-color")).toBe("red");

    interp.getFunction("dom/set-html!")!(div, "<span>inside</span>");
    expect(interp.getFunction("dom/get-html")!(div)).toBe("<span>inside</span>");

    const input = interp.getFunction("dom/create-element")!("input");
    interp.getFunction("dom/set-value!")!(input, "typed");
    expect(interp.getFunction("dom/get-value")!(input)).toBe("typed");
  });

  it("dom/remove-attribute! and dom/remove! mutate the live element", () => {
    document.body.innerHTML = '<div id="root"></div>';
    const handle = interp.getFunction("dom/query")!("#root");

    interp.getFunction("dom/set-attribute!")!(handle, "data-x", "1");
    interp.getFunction("dom/remove-attribute!")!(handle, "data-x");
    expect((ctx.handles.get(handle) as Element).hasAttribute("data-x")).toBe(false);

    interp.getFunction("dom/remove!")!(handle);
    expect(document.querySelector("#root")).toBeNull();
  });

  // --- event-value ---

  it("dom/event-value returns event.target.value", () => {
    // Manually store an event with a target that has .value
    const fakeEvent = new Event("input");
    const input = document.createElement("input");
    input.value = "typed-text";
    Object.defineProperty(fakeEvent, "target", { value: input });

    // Store directly in the handle map
    const evHandle = ctx.nextHandle++;
    ctx.handles.set(evHandle, fakeEvent as any);

    const result = interp.getFunction("dom/event-value")!(evHandle);
    expect(result).toBe("typed-text");
  });

  it("event helpers return null for targets without value, key, or element ancestry", () => {
    const ev = new Event("custom");
    Object.defineProperty(ev, "target", { value: document.createTextNode("x") });
    const evHandle = storeHandle(ev, ctx)!;

    expect(interp.getFunction("dom/event-value")!(evHandle)).toBeNull();
    expect(interp.getFunction("dom/event-key")!(evHandle)).toBeNull();
    expect(interp.getFunction("dom/event-target")!(evHandle)).toBeNull();
    expect(interp.getFunction("dom/event-target-closest")!(evHandle, ".anything")).toBeNull();
  });

  it("event key, target, closest, prevent-default, and stop-propagation expose event details safely", () => {
    document.body.innerHTML = '<div class="card"><button id="btn">go</button></div>';
    const button = document.querySelector("#btn")!;
    const ev = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    Object.defineProperty(ev, "target", { value: button });
    const evHandle = storeHandle(ev, ctx)!;

    expect(interp.getFunction("dom/event-key")!(evHandle)).toBe("Enter");

    const targetHandle = interp.getFunction("dom/event-target")!(evHandle);
    const closestHandle = interp.getFunction("dom/event-target-closest")!(evHandle, ".card");
    expect(ctx.handles.get(targetHandle)).toBe(button);
    expect((ctx.handles.get(closestHandle) as Element).className).toBe("card");

    interp.getFunction("dom/prevent-default!")!(evHandle);
    interp.getFunction("dom/stop-propagation!")!(evHandle);
    expect(ev.defaultPrevented).toBe(true);
    expect((ev as any).__sema_stop).toBe(true);
  });

  it("event target handles are reused across repeated helper calls", () => {
    document.body.innerHTML = '<div class="card"><button id="btn">go</button></div>';
    const button = document.querySelector("#btn")!;
    const ev = new Event("click", { bubbles: true });
    Object.defineProperty(ev, "target", { value: button });
    const evHandle = storeHandle(ev, ctx)!;

    const targetA = interp.getFunction("dom/event-target")!(evHandle);
    const targetB = interp.getFunction("dom/event-target")!(evHandle);
    const cardA = interp.getFunction("dom/event-target-closest")!(evHandle, ".card");
    const cardB = interp.getFunction("dom/event-target-closest")!(evHandle, ".card");

    expect(targetB).toBe(targetA);
    expect(cardB).toBe(cardA);
    expect(ctx.handles.size).toBe(3);
  });

  // --- dom/render returns handle ---

  it("dom/render returns a numeric handle", () => {
    const handle = interp.getFunction("dom/render")!([":div"]);
    expect(typeof handle).toBe("number");
  });

  it("dom/render wraps primitive SIP results in an element handle", () => {
    const handle = interp.getFunction("dom/render")!("plain text");
    const el = ctx.handles.get(handle) as HTMLElement;
    expect(el.tagName).toBe("SPAN");
    expect(el.textContent).toBe("plain text");
  });

  // --- dom/render-into! ---

  it("dom/render-into! renders into target element", () => {
    document.body.innerHTML = '<div id="app"></div>';
    interp.getFunction("dom/render-into!")!("#app", [":p", "Hello"]);
    const app = document.getElementById("app")!;
    expect(app.innerHTML).toBe("<p>Hello</p>");
  });

  it("dom/render-into! throws when the target selector is missing", () => {
    expect(() => interp.getFunction("dom/render-into!")!("#missing", [":p", "x"])).toThrow(/target not found/);
  });

  it("dom/focus! focuses focusable handles and no-ops for non-focusable elements", () => {
    document.body.innerHTML = "";
    const button = interp.getFunction("dom/create-element")!("button");
    document.body.appendChild(ctx.handles.get(button) as Element);
    interp.getFunction("dom/focus!")!(button);
    expect(document.activeElement).toBe(ctx.handles.get(button));

    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    const svgHandle = storeHandle(svg, ctx)!;
    expect(() => interp.getFunction("dom/focus!")!(svgHandle)).not.toThrow();
  });

  // --- dom/on! event handle auto-release ---

  it("event handle is auto-released after dom/on! handler fires", () => {
    const handle = interp.getFunction("dom/create-element")!("button");
    const el = ctx.handles.get(handle) as Element;
    document.body.appendChild(el);

    // Register handler — evalStr is called with the event handle
    interp.getFunction("dom/on!")!(handle, "click", "my-handler");

    // Track which handle IDs exist before click
    const handlesBefore = new Set(ctx.handles.keys());

    // Simulate click
    el.dispatchEvent(new Event("click"));

    // The event handle that was created during dispatch should have been deleted.
    // Any handle created after our snapshot should be gone.
    for (const id of ctx.handles.keys()) {
      if (!handlesBefore.has(id)) {
        // This handle was created during dispatch — it should have been released
        expect.unreachable("Event handle should have been released");
      }
    }
  });

  it("dom/on! accepts direct function callbacks and dom/off! removes them", () => {
    const handle = interp.getFunction("dom/create-element")!("button");
    const el = ctx.handles.get(handle) as Element;
    document.body.appendChild(el);

    const calls: number[] = [];
    const callback = (evHandle: number) => {
      calls.push(evHandle);
    };

    interp.getFunction("dom/on!")!(handle, "click", callback);
    el.dispatchEvent(new Event("click"));
    expect(calls).toHaveLength(1);

    interp.getFunction("dom/off!")!(handle, "click", callback);
    el.dispatchEvent(new Event("click"));
    expect(calls).toHaveLength(1);
  });

  it("dom/on! replaces duplicate listener registrations and updates component ownership", () => {
    const handle = interp.getFunction("dom/create-element")!("button");
    const el = ctx.handles.get(handle) as Element;
    document.body.appendChild(el);
    const component = {
      instanceId: 1,
      target: document.createElement("div"),
      componentFn: "view",
      dispose: null,
      eventCleanup: null,
      localState: new Map(),
      mountCleanup: null,
      pendingMount: null,
      ownedSignalIds: new Set<number>(),
      ownedWatchIds: new Set<number>(),
      ownedIntervalIds: new Set<number>(),
      ownedStreamIds: new Set<number>(),
      ownedListenerKeys: new Set<string>(),
    };
    ctx.mountedComponents.set("#app", component as any);
    ctx.mountedComponentsById.set(component.instanceId, component as any);
    ctx.ownerStack.push(component.instanceId);

    interp.getFunction("dom/on!")!(handle, "click", "handler");
    interp.getFunction("dom/on!")!(handle, "click", "handler");
    ctx.ownerStack.pop();

    expect(ctx.listeners.size).toBe(1);
    expect(component.ownedListenerKeys.size).toBe(1);

    interp.getFunction("dom/off!")!(handle, "click", "handler");
    expect(ctx.listeners.size).toBe(0);
    expect(component.ownedListenerKeys.size).toBe(0);
  });

  // --- dom/on! errors route through ctx.onerror ---

  it("event errors route through ctx.onerror", () => {
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;

    const handle = interp.getFunction("dom/create-element")!("button");
    const el = ctx.handles.get(handle) as Element;
    document.body.appendChild(el);

    interp.invokeGlobal = () => { throw new Error("boom"); };

    interp.getFunction("dom/on!")!(handle, "click", "bad-handler");
    el.dispatchEvent(new Event("click"));

    expect(onerrorSpy).toHaveBeenCalledWith(
      expect.any(Error),
      expect.stringContaining("event:click"),
    );
  });
});
