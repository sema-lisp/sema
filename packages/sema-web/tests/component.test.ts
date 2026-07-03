import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { SemaWebContext } from "../src/context.js";
import { registerDomBindings } from "../src/dom.js";
import { createMockInterpreter } from "./helpers.js";

// Mock morphdom — it doesn't work in jsdom
vi.mock("morphdom", () => ({
  default: vi.fn((fromNode: Element, toNode: Element, opts?: any) => {
    // Simple replacement: copy children from toNode to fromNode
    if (opts?.childrenOnly) {
      fromNode.innerHTML = toNode.innerHTML;
    }
    return fromNode;
  }),
}));

// Mock @preact/signals-core — effect() runs synchronously in tests
vi.mock("@preact/signals-core", () => ({
  signal: (val: any) => ({ value: val, peek: () => val }),
  computed: (fn: () => any) => ({
    get value() {
      return fn();
    },
  }),
  effect: (fn: () => void) => {
    fn();
    return () => {};
  },
  batch: (fn: () => void) => fn(),
}));

// Import after mocks are set up
const { registerComponentBindings, disposeAllComponents } = await import("../src/component.js");
const { registerHttpBindings } = await import("../src/http.js");
const morphdom = (await import("morphdom")).default as any;

describe("registerComponentBindings", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    document.body.innerHTML = '<div id="app"></div><div id="app2"></div>';
    registerComponentBindings(interp, ctx);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  // --- Registration ---

  it("registers component/mount! function", () => {
    expect(interp.getFunction("component/mount!")).toBeDefined();
  });

  it("registers component/unmount! function", () => {
    expect(interp.getFunction("component/unmount!")).toBeDefined();
  });

  it("registers __component/current-id function", () => {
    expect(interp.getFunction("__component/current-id")).toBeDefined();
  });

  // --- mount ---

  it("component/mount! with valid selector registers in ctx.mountedComponents", () => {
    interp.invokeGlobal = (name: string) => {
      if (name === "my-view") return [":div", "hello"];
      return null;
    };

    interp.getFunction("component/mount!")!("#app", "my-view");
    expect(ctx.mountedComponents.has("#app")).toBe(true);
    expect(ctx.mountedComponents.get("#app")!.componentFn).toBe("my-view");
  });

  it("mounting the same component function twice keeps instance-local state isolated", () => {
    interp.registerFunction("shared-view", () => {
      const currentId = interp.getFunction("__component/current-id")!();
      const localId = interp.getFunction("__component/local")!("count", 0);
      return [":div", `${currentId}:${localId}`];
    });

    interp.getFunction("component/mount!")!("#app", "shared-view");
    interp.getFunction("component/mount!")!("#app2", "shared-view");

    const first = ctx.mountedComponents.get("#app")!;
    const second = ctx.mountedComponents.get("#app2")!;

    expect(first.instanceId).not.toBe(second.instanceId);
    expect(first.localState.get("count")).not.toBe(second.localState.get("count"));
  });

  it("remounting runs mount cleanup and tears down delegated listeners", () => {
    const target = document.getElementById("app")!;
    const removeSpy = vi.spyOn(target, "removeEventListener");
    let cleanupCalls = 0;

    interp.registerFunction("mount-cleanup-fn", () => {
      cleanupCalls += 1;
      return null;
    });
    interp.registerFunction("mount-hook", () => "mount-cleanup-fn");
    interp.registerFunction("my-view", () => {
      interp.getFunction("__component/on-mount")!("mount-hook");
      return [":div", "hello"];
    });

    interp.getFunction("component/mount!")!("#app", "my-view");
    interp.getFunction("component/mount!")!("#app", "my-view");

    expect(cleanupCalls).toBe(1);
    expect(removeSpy).toHaveBeenCalled();
  });

  it("on-mount accepts direct function callbacks that return direct cleanup functions", () => {
    let mountCalls = 0;
    let cleanupCalls = 0;

    interp.registerFunction("my-view", () => {
      interp.getFunction("__component/on-mount")!(() => {
        mountCalls += 1;
        return () => {
          cleanupCalls += 1;
        };
      });
      return [":div", "hello"];
    });

    interp.getFunction("component/mount!")!("#app", "my-view");
    interp.getFunction("component/unmount!")!("#app");

    expect(mountCalls).toBe(1);
    expect(cleanupCalls).toBe(1);
  });

  it("mount validates component names and target selectors before registering anything", () => {
    expect(() => interp.getFunction("component/mount!")!("#app", "bad name")).toThrow(/Invalid component function name/);
    expect(() => interp.getFunction("component/mount!")!("#missing", "my-view")).toThrow(/target not found/);
    expect(ctx.mountedComponents.size).toBe(0);
  });

  it("render errors route through ctx.onerror and leave the previous DOM alone", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    document.getElementById("app")!.innerHTML = "<p>stale</p>";
    interp.registerFunction("bad-view", () => {
      throw new Error("render boom");
    });

    interp.getFunction("component/mount!")!("#app", "bad-view");

    expect(document.getElementById("app")!.innerHTML).toBe("<p>stale</p>");
    expect(errors).toEqual([{ message: "render boom", context: "component:bad-view" }]);
  });

  it("force-render recreates the render effect for an already mounted component", () => {
    let renders = 0;
    interp.registerFunction("my-view", () => {
      renders += 1;
      return [":div", `render-${renders}`];
    });

    interp.getFunction("component/mount!")!("#app", "my-view");
    interp.getFunction("component/force-render!")!("#app");
    interp.getFunction("component/force-render!")!("#missing");

    expect(renders).toBe(2);
    expect(document.getElementById("app")!.textContent).toBe("render-2");
  });

  it("morphdom callbacks preserve focused input state and release discarded subtree handles", () => {
    interp.registerFunction("my-view", () => [":input", { ":id": "focused", ":value": "from-state", ":data-x": "1" }]);
    interp.getFunction("component/mount!")!("#app", "my-view");

    const fromInput = document.querySelector("#focused") as HTMLInputElement;
    fromInput.value = "typed";
    fromInput.focus();
    interp.getFunction("component/force-render!")!("#app");

    const opts = morphdom.mock.calls.at(-1)?.[2];
    const toInput = document.createElement("input");
    toInput.setAttribute("value", "from-state");
    toInput.setAttribute("data-x", "2");

    expect(opts.onBeforeElUpdated(fromInput, toInput)).toBe(false);
    expect(fromInput.value).toBe("typed");
    expect(fromInput.getAttribute("data-x")).toBe("2");

    const discarded = document.createElement("span");
    const handleId = ctx.nextHandle++;
    ctx.handles.set(handleId, discarded);
    opts.onNodeDiscarded(discarded);
    expect(ctx.handles.has(handleId)).toBe(false);
  });

  // --- unmount ---

  it("component/unmount! removes from ctx.mountedComponents and clears target", () => {
    interp.invokeGlobal = (name: string) => {
      if (name === "my-view") return [":div", "hello"];
      return null;
    };

    interp.getFunction("component/mount!")!("#app", "my-view");
    expect(ctx.mountedComponents.has("#app")).toBe(true);

    interp.getFunction("component/unmount!")!("#app");
    expect(ctx.mountedComponents.has("#app")).toBe(false);
    expect(document.getElementById("app")!.innerHTML).toBe("");
  });

  it("component/unmount! on non-existent selector is no-op", () => {
    expect(() => {
      interp.getFunction("component/unmount!")!("#nonexistent");
    }).not.toThrow();
  });

  // --- __component/current-id ---

  it("__component/current-id returns null when no component is rendering", () => {
    const result = interp.getFunction("__component/current-id")!();
    expect(result).toBeNull();
  });

  it("__component/local and __component/on-mount throw outside render context", () => {
    expect(() => interp.getFunction("__component/local")!("x", 0)).toThrow(/outside of a component render context/);
    expect(() => interp.getFunction("__component/on-mount")!(() => {})).toThrow(/outside of a component render context/);
  });

  it("delegated click handlers receive a temporary event handle and release it afterward", () => {
    let calls = 0;
    let eventHandleDuringCall: number | null = null;
    interp.registerFunction("handle-click", (evHandle: number) => {
      calls += 1;
      eventHandleDuringCall = evHandle;
      expect(ctx.handles.get(evHandle)).toBeInstanceOf(Event);
    });
    interp.registerFunction("button-view", () => [":button", { ":id": "btn", ":on-click": "handle-click" }, "go"]);

    interp.getFunction("component/mount!")!("#app", "button-view");
    const handlesBefore = new Set(ctx.handles.keys());
    document.querySelector("#btn")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));

    expect(calls).toBe(1);
    expect(eventHandleDuringCall).not.toBeNull();
    expect(ctx.handles.has(eventHandleDuringCall!)).toBe(false);
    for (const id of ctx.handles.keys()) {
      expect(handlesBefore.has(id)).toBe(true);
    }
  });

  it("delegated events bubble to ancestor handlers and respect stop-propagation", () => {
    registerDomBindings(interp, ctx);
    const calls: string[] = [];
    interp.registerFunction("child-click", (evHandle: number) => {
      calls.push("child");
      interp.getFunction("dom/stop-propagation!")!(evHandle);
    });
    interp.registerFunction("parent-click", () => {
      calls.push("parent");
    });
    interp.registerFunction("nested-view", () => [
      ":div",
      { ":id": "parent", ":on-click": "parent-click" },
      [":button", { ":id": "child", ":on-click": "child-click" }, "go"],
    ]);

    interp.getFunction("component/mount!")!("#app", "nested-view");
    document.querySelector("#child")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));

    expect(calls).toEqual(["child"]);
  });

  it("delegated mouseenter and mouseleave ignore internal movement but dispatch boundary crossings", () => {
    const calls: string[] = [];
    interp.registerFunction("enter", () => calls.push("enter"));
    interp.registerFunction("leave", () => calls.push("leave"));
    interp.registerFunction("hover-view", () => [
      ":div",
      { ":id": "outer", ":on-mouseenter": "enter", ":on-mouseleave": "leave" },
      [":span", { ":id": "inner" }, "inside"],
    ]);

    interp.getFunction("component/mount!")!("#app", "hover-view");
    const outer = document.querySelector("#outer")!;
    const inner = document.querySelector("#inner")!;

    inner.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: outer }));
    inner.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: outer }));
    expect(calls).toEqual([]);

    inner.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body }));
    inner.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }));
    expect(calls).toEqual(["enter", "leave"]);
  });

  it("delegated event errors route through ctx.onerror", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    interp.registerFunction("boom", () => {
      throw new Error("handler boom");
    });
    interp.registerFunction("button-view", () => [":button", { ":id": "btn", ":on-click": "boom" }, "go"]);

    interp.getFunction("component/mount!")!("#app", "button-view");
    document.querySelector("#btn")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));

    expect(errors).toEqual([{ message: "handler boom", context: "event:click:boom" }]);
  });

  it("interval callbacks route errors, attach to the owner, and can be cleared on unmount", () => {
    vi.useFakeTimers();
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    let calls = 0;

    interp.registerFunction("interval-view", () => {
      interp.getFunction("__component/on-mount")!(() => {
        const id = interp.getFunction("js/set-interval")!(() => {
          calls += 1;
          throw new Error("tick boom");
        }, 10);
        return () => interp.getFunction("js/clear-interval")!(id);
      });
      return [":div", "timer"];
    });

    interp.getFunction("component/mount!")!("#app", "interval-view");
    const component = ctx.mountedComponents.get("#app")!;
    expect(component.ownedIntervalIds.size).toBe(1);

    vi.advanceTimersByTime(10);
    expect(calls).toBe(1);
    expect(errors).toEqual([{ message: "tick boom", context: "interval" }]);

    interp.getFunction("component/unmount!")!("#app");
    vi.advanceTimersByTime(20);
    expect(calls).toBe(1);
    vi.useRealTimers();
  });

  // --- defcomponent macro registration ---

  it("evalStr was called with defcomponent macro definition", () => {
    const calls = interp.getEvalCalls();
    const hasDef = calls.some((c: string) => c.includes("defcomponent"));
    expect(hasDef).toBe(true);
  });

  // --- mount! Sema wrapper registration ---

  it("evalStr was called with mount! wrapper definition", () => {
    const calls = interp.getEvalCalls();
    const hasMount = calls.some((c: string) => c.includes("defmacro mount!"));
    expect(hasMount).toBe(true);
  });

  // --- dispose / ownership edge cases ---

  describe("dispose idempotency and error boundaries", () => {
    it("component/unmount! is idempotent — calling twice doesn't throw or double-run cleanup", () => {
      let mountCalls = 0;
      let cleanupCalls = 0;
      let disposeCalls = 0;

      interp.registerFunction("my-view", () => {
        interp.getFunction("__component/on-mount")!(() => {
          mountCalls += 1;
          return () => {
            cleanupCalls += 1;
          };
        });
        return [":div", "hello"];
      });

      interp.getFunction("component/mount!")!("#app", "my-view");

      const component = ctx.mountedComponents.get("#app")!;
      const originalDispose = component.dispose!;
      component.dispose = () => {
        disposeCalls += 1;
        originalDispose();
      };

      expect(() => {
        interp.getFunction("component/unmount!")!("#app");
        interp.getFunction("component/unmount!")!("#app");
      }).not.toThrow();

      expect(mountCalls).toBe(1);
      expect(cleanupCalls).toBe(1);
      expect(disposeCalls).toBe(1);
      expect(ctx.mountedComponents.has("#app")).toBe(false);
    });

    it("disposeAllComponents is idempotent across repeated calls", () => {
      let cleanupCalls = 0;

      interp.registerFunction("my-view", () => {
        interp.getFunction("__component/on-mount")!(() => {
          return () => {
            cleanupCalls += 1;
          };
        });
        return [":div", "hello"];
      });

      interp.getFunction("component/mount!")!("#app", "my-view");

      expect(() => {
        disposeAllComponents(ctx);
        disposeAllComponents(ctx);
      }).not.toThrow();

      expect(cleanupCalls).toBe(1);
      expect(ctx.mountedComponents.size).toBe(0);
      expect(ctx.mountedComponentsById.size).toBe(0);
    });

    it("dispose: a throwing cleanup stage still lets sibling cleanup stages run and routes errors through ctx.onerror", () => {
      const errors: Array<{ message: string; context: string }> = [];
      ctx.onerror = (e, context) => errors.push({ message: e.message, context });

      interp.registerFunction("my-view", () => [":div", "hello"]);
      interp.getFunction("component/mount!")!("#app", "my-view");

      const component = ctx.mountedComponents.get("#app")!;

      let mountCleanupCalled = false;
      component.mountCleanup = () => {
        mountCleanupCalled = true;
        throw new Error("mount-cleanup boom");
      };

      let disposeCalled = false;
      const originalDispose = component.dispose!;
      component.dispose = () => {
        disposeCalled = true;
        originalDispose();
        throw new Error("effect-dispose boom");
      };

      let eventCleanupCalled = false;
      const originalEventCleanup = component.eventCleanup!;
      component.eventCleanup = () => {
        eventCleanupCalled = true;
        originalEventCleanup();
        throw new Error("event-cleanup boom");
      };

      expect(() => {
        interp.getFunction("component/unmount!")!("#app");
      }).not.toThrow();

      // All three cleanup stages ran despite each one throwing.
      expect(mountCleanupCalled).toBe(true);
      expect(disposeCalled).toBe(true);
      expect(eventCleanupCalled).toBe(true);

      expect(errors).toEqual([
        { message: "mount-cleanup boom", context: "unmount-cleanup:my-view" },
        { message: "effect-dispose boom", context: "component-dispose:my-view" },
        { message: "event-cleanup boom", context: "component-events:my-view" },
      ]);

      // The component is still fully torn down despite every cleanup stage throwing.
      expect(ctx.mountedComponents.has("#app")).toBe(false);
      expect(ctx.mountedComponentsById.has(component.instanceId)).toBe(false);
      expect(document.getElementById("app")!.innerHTML).toBe("");
    });

    it("dispose: a throwing owned-resource cleanup (listener/watch/interval/stream) still lets sibling owned resources clean up and routes errors through ctx.onerror", () => {
      const errors: Array<{ message: string; context: string }> = [];
      ctx.onerror = (e, context) => errors.push({ message: e.message, context });

      interp.registerFunction("my-view", () => [":div", "hello"]);
      interp.getFunction("component/mount!")!("#app", "my-view");

      const component = ctx.mountedComponents.get("#app")!;

      // Rig one owned resource of each kind to throw on cleanup, and one
      // "healthy" resource of each kind to confirm siblings still clean up.
      const throwingTarget = { removeEventListener: vi.fn(() => { throw new Error("listener boom"); }) } as unknown as EventTarget;
      const okListener = vi.fn();
      ctx.listeners.set("listener-throw", { target: throwingTarget, event: "click", listener: vi.fn() });
      ctx.listeners.set("listener-ok", { target: { removeEventListener: okListener } as unknown as EventTarget, event: "click", listener: vi.fn() });
      component.ownedListenerKeys.add("listener-throw");
      component.ownedListenerKeys.add("listener-ok");

      const okWatchDispose = vi.fn();
      ctx.watchDisposers.set(101, { dispose: () => { throw new Error("watch boom"); } });
      ctx.watchDisposers.set(102, { dispose: okWatchDispose });
      component.ownedWatchIds.add(101);
      component.ownedWatchIds.add(102);

      const okStreamClose = vi.fn();
      ctx.streams.set(201, { kind: "llm-stream", close: () => { throw new Error("stream boom"); } });
      ctx.streams.set(202, { kind: "llm-stream", close: okStreamClose });
      component.ownedStreamIds.add(201);
      component.ownedStreamIds.add(202);

      expect(() => {
        interp.getFunction("component/unmount!")!("#app");
      }).not.toThrow();

      // Siblings of the throwing resource still ran their cleanup.
      expect(okListener).toHaveBeenCalledTimes(1);
      expect(okWatchDispose).toHaveBeenCalledTimes(1);
      expect(okStreamClose).toHaveBeenCalledTimes(1);

      expect(errors).toEqual(
        expect.arrayContaining([
          { message: "listener boom", context: "component-listener-cleanup:my-view" },
          { message: "watch boom", context: "component-watch-cleanup:my-view" },
          { message: "stream boom", context: "component-stream-cleanup:my-view" },
        ]),
      );

      // The component is still fully torn down despite every owned-resource kind throwing.
      expect(ctx.mountedComponents.has("#app")).toBe(false);
      expect(ctx.mountedComponentsById.has(component.instanceId)).toBe(false);
      expect(document.getElementById("app")!.innerHTML).toBe("");
    });

    it("disposeAllComponents tears down every component even when one throws mid-teardown", () => {
      const errors: Array<{ message: string; context: string }> = [];
      ctx.onerror = (e, context) => errors.push({ message: e.message, context });

      interp.registerFunction("boom-view", () => [":div", "boom"]);
      interp.registerFunction("fine-view", () => [":div", "fine"]);
      interp.getFunction("component/mount!")!("#app", "boom-view");
      interp.getFunction("component/mount!")!("#app2", "fine-view");

      const boomComponent = ctx.mountedComponents.get("#app")!;
      boomComponent.dispose = () => {
        throw new Error("dispose boom");
      };

      expect(() => disposeAllComponents(ctx)).not.toThrow();

      // Both components were torn down despite the first one throwing.
      expect(ctx.mountedComponents.has("#app")).toBe(false);
      expect(ctx.mountedComponents.has("#app2")).toBe(false);
      expect(document.getElementById("app")!.innerHTML).toBe("");
      expect(document.getElementById("app2")!.innerHTML).toBe("");
      expect(errors.some(e => e.context === "component-dispose:boom-view")).toBe(true);
    });

    it("disposing a component mid-stream aborts the underlying request and stops further state updates", async () => {
      registerHttpBindings(interp, ctx);

      let aborted = false;
      const fetchMock = vi.fn().mockImplementation((_url: string, init?: RequestInit) => {
        init?.signal?.addEventListener("abort", () => {
          aborted = true;
        });
        return Promise.resolve(
          new Response(
            new ReadableStream({
              start() {
                // Leave open until aborted — simulates an in-flight stream.
              },
            }),
            {
              status: 200,
              headers: { "Content-Type": "text/event-stream" },
            },
          ),
        );
      });
      vi.stubGlobal("fetch", fetchMock);

      const errors: Array<{ message: string; context: string }> = [];
      ctx.onerror = (e, context) => errors.push({ message: e.message, context });

      let streamSignalId: number | null = null;
      interp.registerFunction("stream-view", () => {
        streamSignalId = interp.getFunction("http/event-source")!("/stream");
        return [":div", "streaming"];
      });

      interp.getFunction("component/mount!")!("#app", "stream-view");

      expect(streamSignalId).not.toBeNull();
      const component = ctx.mountedComponents.get("#app")!;
      expect(component.ownedStreamIds.has(streamSignalId!)).toBe(true);
      expect(ctx.streams.has(streamSignalId!)).toBe(true);
      expect(ctx.signals.has(streamSignalId!)).toBe(true);

      // Dispose the component while the stream is still in flight (never closed/aborted yet).
      interp.getFunction("component/unmount!")!("#app");

      await Promise.resolve();
      await new Promise((resolve) => setTimeout(resolve, 0));

      // The underlying fetch was aborted and the stream/signal were torn down
      // as part of the component's owner-cascade cleanup.
      expect(aborted).toBe(true);
      expect(ctx.streams.has(streamSignalId!)).toBe(false);
      expect(ctx.signals.has(streamSignalId!)).toBe(false);
      expect(component.ownedStreamIds.size).toBe(0);

      // Simulate the aborted stream still trying to deliver data after dispose —
      // there must be no tracked signal left for it to update into, and no
      // error/warning should have been raised by the teardown itself.
      expect(ctx.signals.get(streamSignalId!)).toBeUndefined();
      expect(errors).toEqual([]);
    });
  });
});
