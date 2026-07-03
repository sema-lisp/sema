import { describe, it, expect, vi } from "vitest";
import { signal, effect } from "@preact/signals-core";
import { registerReactiveBindings } from "../src/reactive.js";
import { SemaWebContext, disposeContextResources } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";

function setup() {
  const interp = createMockInterpreter();
  const ctx = new SemaWebContext();
  registerReactiveBindings(interp, ctx);
  return { interp, ctx };
}

describe("registerReactiveBindings", () => {
  it("registers __state/create, __state/deref, __state/put! functions", () => {
    const { interp } = setup();

    expect(interp.getFunction("__state/create")).toBeDefined();
    expect(interp.getFunction("__state/deref")).toBeDefined();
    expect(interp.getFunction("__state/put!")).toBeDefined();
  });

  it("__state/create returns numeric ID, __state/deref reads value", () => {
    const { interp } = setup();

    const create = interp.getFunction("__state/create")!;
    const deref = interp.getFunction("__state/deref")!;

    const id = create(42);
    expect(typeof id).toBe("number");
    expect(id).toBe(1);
    expect(deref(id)).toBe(42);
  });

  it("__state/put! changes value", () => {
    const { interp } = setup();

    const create = interp.getFunction("__state/create")!;
    const deref = interp.getFunction("__state/deref")!;
    const put = interp.getFunction("__state/put!")!;

    const id = create(42);
    put(id, 99);
    expect(deref(id)).toBe(99);
  });

  it("__state/deref throws on unknown ID", () => {
    const { interp } = setup();
    const deref = interp.getFunction("__state/deref")!;

    expect(() => deref(999)).toThrow("Unknown state");
  });

  it("__state/put! throws on unknown ID", () => {
    const { interp } = setup();
    const put = interp.getFunction("__state/put!")!;

    expect(() => put(999, 1)).toThrow("Unknown state");
  });

  it("multiple signals are independent", () => {
    const { interp } = setup();

    const create = interp.getFunction("__state/create")!;
    const deref = interp.getFunction("__state/deref")!;
    const put = interp.getFunction("__state/put!")!;

    const id1 = create("hello");
    const id2 = create("world");

    expect(id1).not.toBe(id2);
    expect(deref(id1)).toBe("hello");
    expect(deref(id2)).toBe("world");

    put(id1, "changed");
    expect(deref(id1)).toBe("changed");
    expect(deref(id2)).toBe("world");
  });

  it("__state/watch calls back when value changes", () => {
    const { interp } = setup();

    const create = interp.getFunction("__state/create")!;
    const put = interp.getFunction("__state/put!")!;
    const watch = interp.getFunction("__state/watch")!;

    const id = create(10);
    const callsBefore = interp.getEvalCalls().length;

    const watchId = watch(id, "my-callback");
    expect(typeof watchId).toBe("number");

    // Change the value to trigger the watch effect
    put(id, 20);

    // The watch should have called evalStr with the callback
    const callsAfter = interp.getEvalCalls();
    const watchCalls = callsAfter.slice(callsBefore).filter(
      (c) => c.includes("my-callback")
    );
    expect(watchCalls.length).toBeGreaterThan(0);
  });

  it("__state/unwatch stops further callbacks", () => {
    const { interp } = setup();

    const create = interp.getFunction("__state/create")!;
    const put = interp.getFunction("__state/put!")!;
    const watch = interp.getFunction("__state/watch")!;
    const unwatch = interp.getFunction("__state/unwatch")!;

    const id = create(10);
    const watchId = watch(id, "my-callback");

    put(id, 20);
    const callsAfterFirstUpdate = interp.getEvalCalls().filter((c) => c.includes("my-callback")).length;

    unwatch(watchId);
    put(id, 30);

    const callsAfterUnwatch = interp.getEvalCalls().filter((c) => c.includes("my-callback")).length;
    expect(callsAfterFirstUpdate).toBeGreaterThan(0);
    expect(callsAfterUnwatch).toBe(callsAfterFirstUpdate);
  });

  it("__state/watch accepts direct function callbacks", () => {
    const { interp } = setup();

    const create = interp.getFunction("__state/create")!;
    const put = interp.getFunction("__state/put!")!;
    const watch = interp.getFunction("__state/watch")!;

    const id = create(1);
    const seen: Array<[number, number]> = [];

    watch(id, (oldVal: number, newVal: number) => {
      seen.push([oldVal, newVal]);
    });

    put(id, 2);
    put(id, 3);

    expect(seen).toEqual([[1, 2], [2, 3]]);
  });

  it("__state/watch throws on unknown state IDs", () => {
    const { interp } = setup();

    expect(() => interp.getFunction("__state/watch")!(999, () => {})).toThrow("Unknown state");
  });

  it("__state/watch routes callback errors through ctx.onerror and continues tracking", () => {
    const { interp, ctx } = setup();
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;
    const create = interp.getFunction("__state/create")!;
    const put = interp.getFunction("__state/put!")!;
    const watch = interp.getFunction("__state/watch")!;
    const id = create(1);

    watch(id, () => {
      throw new Error("watch boom");
    });
    put(id, 2);
    put(id, 3);

    expect(onerrorSpy).toHaveBeenCalledTimes(2);
    expect(onerrorSpy).toHaveBeenCalledWith(expect.any(Error), "watch");
  });

  it("__state/watch binds ownership from the current execution context", () => {
    const { interp, ctx } = setup();

    const create = interp.getFunction("__state/create")!;
    const watch = interp.getFunction("__state/watch")!;

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
    ctx.mountedComponentsById.set(component.instanceId, component as any);
    ctx.ownerStack.push(component.instanceId);

    const id = create(1);
    const watchId = watch(id, () => {});

    ctx.ownerStack.pop();

    expect(component.ownedWatchIds.has(watchId)).toBe(true);
  });

  it("__state/batch-run executes the thunk via evalStr", () => {
    const { interp } = setup();

    const batchRun = interp.getFunction("__state/batch-run")!;
    const callsBefore = interp.getEvalCalls().length;

    batchRun("my_thunk");

    const callsAfter = interp.getEvalCalls();
    const batchCalls = callsAfter.slice(callsBefore).filter(
      (c) => c.includes("my_thunk")
    );
    expect(batchCalls.length).toBeGreaterThan(0);
  });

  it("__state/batch-run routes callback errors through ctx.onerror and releases direct callbacks", () => {
    const { interp, ctx } = setup();
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;
    let released = 0;
    const callback = Object.assign(() => {
      throw new Error("batch boom");
    }, {
      __semaRelease: () => {
        released += 1;
      },
    });

    expect(interp.getFunction("__state/batch-run")!(callback)).toBeUndefined();

    expect(onerrorSpy).toHaveBeenCalledWith(expect.any(Error), "batch");
    expect(released).toBe(1);
  });

  it("__state/computed-create creates a computed signal via evalStr", () => {
    const { interp, ctx } = setup();

    const computedCreate = interp.getFunction("__state/computed-create")!;
    const deref = interp.getFunction("__state/deref")!;
    const callsBefore = interp.getEvalCalls().length;

    const id = computedCreate("my_thunk");
    expect(typeof id).toBe("number");

    // Computed signals are lazy -- reading the value triggers evaluation
    deref(id);

    const callsAfter = interp.getEvalCalls();
    const computedCalls = callsAfter.slice(callsBefore).filter(
      (c) => c.includes("my_thunk")
    );
    expect(computedCalls.length).toBeGreaterThan(0);
  });

  it("__state/computed-create routes callback errors through ctx.onerror and returns undefined", () => {
    const { interp, ctx } = setup();
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;
    const id = interp.getFunction("__state/computed-create")!(() => {
      throw new Error("computed boom");
    });

    expect(interp.getFunction("__state/deref")!(id)).toBeUndefined();
    expect(onerrorSpy).toHaveBeenCalledWith(expect.any(Error), "computed");
  });

  it("releases computed callback handles when the context is disposed", () => {
    const { interp, ctx } = setup();

    const computedCreate = interp.getFunction("__state/computed-create")!;
    let released = 0;
    const callback = Object.assign(() => 42, {
      __semaRelease: () => {
        released += 1;
      },
    });

    const id = computedCreate(callback);
    expect(ctx.signals.has(id)).toBe(true);

    disposeContextResources(ctx);

    expect(released).toBe(1);
    expect(ctx.signals.has(id)).toBe(false);
  });

  it("Sema wrappers are registered via evalStr", () => {
    const { interp } = setup();

    const evalCalls = interp.getEvalCalls();
    // The registration should have called evalStr with the wrapper definitions
    const wrapperCall = evalCalls.find(
      (c) => c.includes("state/deref") || c.includes("deref ref")
    );
    expect(wrapperCall).toBeDefined();

    // Check that put! and update! wrappers are included
    const putCall = evalCalls.find((c) => c.includes("put!"));
    expect(putCall).toBeDefined();
  });

  it("throws a clear setup error when reactive wrapper registration fails", () => {
    const interp = createMockInterpreter();
    interp.evalStr = () => ({ value: null, output: [], error: "reactive wrapper boom" });

    expect(() => registerReactiveBindings(interp, new SemaWebContext())).toThrow(/reactive wrapper boom/);
  });
});

describe("@preact/signals-core integration", () => {
  it("signal and effect work together", () => {
    const s = signal(0);
    const values: number[] = [];

    const dispose = effect(() => {
      values.push(s.value);
    });

    s.value = 1;
    s.value = 2;

    expect(values).toEqual([0, 1, 2]);
    dispose();
  });
});
