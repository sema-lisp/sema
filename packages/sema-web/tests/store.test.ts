import { describe, it, expect, beforeEach, vi } from "vitest";
import { registerStoreBindings } from "../src/store.js";
import { SemaWebContext } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";

describe("registerStoreBindings", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    registerStoreBindings(interp, ctx);
  });

  // --- localStorage: number ---

  it("store/set! then store/get round-trips a number", () => {
    interp.getFunction("store/set!")!("key", 42);
    const result = interp.getFunction("store/get")!("key");
    expect(result).toBe(42);
  });

  // --- localStorage: string ---

  it("store/set! then store/get round-trips a string", () => {
    interp.getFunction("store/set!")!("key", "hello");
    const result = interp.getFunction("store/get")!("key");
    expect(result).toBe("hello");
  });

  // --- Type fidelity: string "42" stays string ---

  it('store/set!("key", "42") returns "42" as string, not number', () => {
    interp.getFunction("store/set!")!("key", "42");
    const result = interp.getFunction("store/get")!("key");
    expect(result).toBe("42");
    expect(typeof result).toBe("string");
  });

  // --- store/has? ---

  it("store/has? returns true after set", () => {
    interp.getFunction("store/set!")!("key", 1);
    expect(interp.getFunction("store/has?")!("key")).toBe(true);
  });

  it("store/has? returns false for missing key", () => {
    expect(interp.getFunction("store/has?")!("nope")).toBe(false);
  });

  // --- store/remove! ---

  it("store/remove! removes the key", () => {
    interp.getFunction("store/set!")!("key", "val");
    interp.getFunction("store/remove!")!("key");
    expect(interp.getFunction("store/get")!("key")).toBeNull();
  });

  // --- store/keys ---

  it("store/keys returns an array", () => {
    interp.getFunction("store/set!")!("a", 1);
    interp.getFunction("store/set!")!("b", 2);
    const keys = interp.getFunction("store/keys")!();
    expect(Array.isArray(keys)).toBe(true);
    expect(keys).toContain("a");
    expect(keys).toContain("b");
  });

  // --- store/clear! ---

  it("store/clear! removes all keys", () => {
    interp.getFunction("store/set!")!("a", 1);
    interp.getFunction("store/set!")!("b", 2);
    interp.getFunction("store/clear!")!();
    expect(interp.getFunction("store/keys")!().length).toBe(0);
  });

  it("store/get returns null and reports corrupted JSON", () => {
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;
    localStorage.setItem("bad", "{not json");

    expect(interp.getFunction("store/get")!("bad")).toBeNull();
    expect(onerrorSpy).toHaveBeenCalledWith(expect.any(SyntaxError), "store/get:bad");
  });

  it("store/set! reports values that cannot be JSON serialized", () => {
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;
    const circular: any = {};
    circular.self = circular;

    expect(interp.getFunction("store/set!")!("circular", circular)).toBeNull();
    expect(onerrorSpy).toHaveBeenCalledWith(expect.any(TypeError), "store/set!:circular");
    expect(localStorage.getItem("circular")).toBeNull();
  });

  // --- sessionStorage ---

  it("store/session-set! and store/session-get round-trip", () => {
    interp.getFunction("store/session-set!")!("skey", "sval");
    const result = interp.getFunction("store/session-get")!("skey");
    expect(result).toBe("sval");
  });

  it("store/session-remove! removes session key", () => {
    interp.getFunction("store/session-set!")!("skey", "sval");
    interp.getFunction("store/session-remove!")!("skey");
    expect(interp.getFunction("store/session-get")!("skey")).toBeNull();
  });

  it("store/session-clear! clears session storage", () => {
    interp.getFunction("store/session-set!")!("a", 1);
    interp.getFunction("store/session-clear!")!();
    expect(interp.getFunction("store/session-get")!("a")).toBeNull();
  });

  it("store/session-get returns null and reports corrupted JSON", () => {
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;
    sessionStorage.setItem("bad", "[");

    expect(interp.getFunction("store/session-get")!("bad")).toBeNull();
    expect(onerrorSpy).toHaveBeenCalledWith(expect.any(SyntaxError), "store/session-get:bad");
  });

  it("store/session-set! reports values that cannot be JSON serialized", () => {
    const onerrorSpy = vi.fn();
    ctx.onerror = onerrorSpy;
    const circular: any = {};
    circular.self = circular;

    expect(interp.getFunction("store/session-set!")!("circular", circular)).toBeNull();
    expect(onerrorSpy).toHaveBeenCalledWith(expect.any(TypeError), "store/session-set!:circular");
    expect(sessionStorage.getItem("circular")).toBeNull();
  });
});
