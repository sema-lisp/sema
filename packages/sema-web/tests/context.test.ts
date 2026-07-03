import { describe, it, expect, vi } from "vitest";
import { SemaWebContext, disposeContextResources } from "../src/context.js";

describe("SemaWebContext", () => {
  it("two instances have independent handles state", () => {
    const ctx1 = new SemaWebContext();
    const ctx2 = new SemaWebContext();

    ctx1.handles.set(1, document.createElement("div"));
    expect(ctx1.handles.size).toBe(1);
    expect(ctx2.handles.size).toBe(0);
  });

  it("handle IDs start at 1 per instance", () => {
    const ctx1 = new SemaWebContext();
    const ctx2 = new SemaWebContext();

    expect(ctx1.nextHandle).toBe(1);
    expect(ctx2.nextHandle).toBe(1);

    ctx1.nextHandle++;
    expect(ctx1.nextHandle).toBe(2);
    expect(ctx2.nextHandle).toBe(1);
  });

  it("signal IDs start at 1 per instance", () => {
    const ctx1 = new SemaWebContext();
    const ctx2 = new SemaWebContext();

    expect(ctx1.nextSignalId).toBe(1);
    expect(ctx2.nextSignalId).toBe(1);

    ctx1.nextSignalId++;
    expect(ctx1.nextSignalId).toBe(2);
    expect(ctx2.nextSignalId).toBe(1);
  });

  it("mountedComponents are independent", () => {
    const ctx1 = new SemaWebContext();
    const ctx2 = new SemaWebContext();

    ctx1.mountedComponents.set("app", {
      instanceId: 1,
      target: document.createElement("div"),
      componentFn: "render-app",
      dispose: null,
      eventCleanup: null,
      localState: new Map(),
      mountCleanup: null,
      pendingMount: null,
      ownedSignalIds: new Set(),
      ownedWatchIds: new Set(),
      ownedIntervalIds: new Set(),
      ownedStreamIds: new Set(),
      ownedListenerKeys: new Set(),
    });

    expect(ctx1.mountedComponents.size).toBe(1);
    expect(ctx2.mountedComponents.size).toBe(0);
  });

  it("default onerror calls console.error", () => {
    const ctx = new SemaWebContext();
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});

    const error = new Error("test error");
    ctx.onerror(error, "test-context");

    expect(spy).toHaveBeenCalledOnce();
    expect(spy).toHaveBeenCalledWith("[sema-web] Error in test-context:", error);

    spy.mockRestore();
  });

  describe("disposeContextResources", () => {
    it("is idempotent — calling twice doesn't throw or double-run cleanup callbacks", () => {
      const ctx = new SemaWebContext();

      const target = document.createElement("div");
      const listener = () => {};
      target.addEventListener("click", listener);
      const removeSpy = vi.spyOn(target, "removeEventListener");
      ctx.listeners.set("click:1", { target, event: "click", listener });

      let watchDisposeCalls = 0;
      ctx.watchDisposers.set(1, { dispose: () => { watchDisposeCalls += 1; } });

      const intervalId = setInterval(() => {}, 1000) as unknown as number;
      const clearSpy = vi.spyOn(globalThis, "clearInterval");
      ctx.intervals.set(intervalId, {});

      let streamCloseCalls = 0;
      ctx.streams.set(1, { kind: "event-source", close: () => { streamCloseCalls += 1; } });

      let cleanupHookCalls = 0;
      ctx.cleanupHooks.add(() => { cleanupHookCalls += 1; });

      ctx.signals.set(1, { value: 1, peek: () => 1 } as any);

      const styleEl = document.createElement("style");
      document.head.appendChild(styleEl);
      ctx.styleEl = styleEl;

      expect(() => disposeContextResources(ctx)).not.toThrow();
      expect(() => disposeContextResources(ctx)).not.toThrow();

      expect(removeSpy).toHaveBeenCalledTimes(1);
      expect(watchDisposeCalls).toBe(1);
      expect(clearSpy).toHaveBeenCalledWith(intervalId);
      expect(clearSpy).toHaveBeenCalledTimes(1);
      expect(streamCloseCalls).toBe(1);
      expect(cleanupHookCalls).toBe(1);
      expect(ctx.signals.size).toBe(0);
      expect(ctx.styleEl).toBeNull();
      expect(ctx.listeners.size).toBe(0);
      expect(ctx.watchDisposers.size).toBe(0);
      expect(ctx.intervals.size).toBe(0);
      expect(ctx.streams.size).toBe(0);
      expect(ctx.cleanupHooks.size).toBe(0);

      clearSpy.mockRestore();
    });

    it("routes a throwing cleanup to ctx.onerror without blocking sibling cleanups", () => {
      const ctx = new SemaWebContext();
      const errors: Array<{ message: string; context: string }> = [];
      ctx.onerror = (e, context) => errors.push({ message: e.message, context });

      const throwingTarget = {
        removeEventListener: () => {
          throw new Error("listener-cleanup boom");
        },
      } as unknown as EventTarget;
      ctx.listeners.set("bad", { target: throwingTarget, event: "click", listener: () => {} });

      let goodListenerRemoved = false;
      const goodTarget = {
        removeEventListener: () => {
          goodListenerRemoved = true;
        },
      } as unknown as EventTarget;
      ctx.listeners.set("good", { target: goodTarget, event: "click", listener: () => {} });

      ctx.watchDisposers.set(1, {
        dispose: () => {
          throw new Error("watch-cleanup boom");
        },
      });

      let cleanupHookRan = false;
      ctx.cleanupHooks.add(() => {
        throw new Error("runtime-cleanup boom");
      });
      ctx.cleanupHooks.add(() => {
        cleanupHookRan = true;
      });

      expect(() => disposeContextResources(ctx)).not.toThrow();

      expect(goodListenerRemoved).toBe(true);
      expect(cleanupHookRan).toBe(true);
      expect(ctx.listeners.size).toBe(0);
      expect(ctx.watchDisposers.size).toBe(0);
      expect(ctx.cleanupHooks.size).toBe(0);

      const contexts = errors.map((e) => e.context);
      expect(contexts).toContain("listener-cleanup:click");
      expect(contexts).toContain("watch-cleanup");
      expect(contexts).toContain("runtime-cleanup");
    });
  });
});
