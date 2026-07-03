import { describe, it, expect } from "vitest";
import { storeHandle, getElement, getNode, getEvent, releaseHandle, SEMA_IDENT_RE } from "../src/handles.js";
import { SemaWebContext } from "../src/context.js";

describe("handles", () => {
  it("storeHandle(null) returns null", () => {
    const ctx = new SemaWebContext();
    expect(storeHandle(null, ctx)).toBeNull();
  });

  it("storeHandle returns incrementing numeric IDs", () => {
    const ctx = new SemaWebContext();
    const el1 = document.createElement("div");
    const el2 = document.createElement("span");

    const id1 = storeHandle(el1, ctx);
    const id2 = storeHandle(el2, ctx);

    expect(id1).toBe(1);
    expect(id2).toBe(2);
  });

  it("storeHandle reuses the existing ID for the same DOM object", () => {
    const ctx = new SemaWebContext();
    const el = document.createElement("div");

    const id1 = storeHandle(el, ctx);
    const id2 = storeHandle(el, ctx);

    expect(id2).toBe(id1);
    expect(ctx.handles.size).toBe(1);
  });

  it("getElement retrieves a stored element", () => {
    const ctx = new SemaWebContext();
    const el = document.createElement("div");
    const id = storeHandle(el, ctx)!;

    expect(getElement(id, ctx)).toBe(el);
  });

  it("getElement throws on unknown handle", () => {
    const ctx = new SemaWebContext();
    expect(() => getElement(999, ctx)).toThrow("Invalid element handle: 999");
  });

  it("getNode on an Event throws", () => {
    const ctx = new SemaWebContext();
    const ev = new Event("click");
    const id = storeHandle(ev, ctx)!;

    expect(() => getNode(id, ctx)).toThrow("Invalid node handle");
  });

  it("getEvent on an Element throws", () => {
    const ctx = new SemaWebContext();
    const el = document.createElement("div");
    const id = storeHandle(el, ctx)!;

    expect(() => getEvent(id, ctx)).toThrow("Invalid event handle");
  });

  it("releaseHandle removes the handle", () => {
    const ctx = new SemaWebContext();
    const el = document.createElement("div");
    const id = storeHandle(el, ctx)!;

    releaseHandle(id, ctx);
    expect(() => getElement(id, ctx)).toThrow("Invalid element handle");
  });

  it("two contexts have independent handle IDs (both start at 1)", () => {
    const ctx1 = new SemaWebContext();
    const ctx2 = new SemaWebContext();

    const id1 = storeHandle(document.createElement("a"), ctx1);
    const id2 = storeHandle(document.createElement("b"), ctx2);

    expect(id1).toBe(1);
    expect(id2).toBe(1);

    // They are independent: getting from wrong context fails
    expect(() => getElement(id1!, ctx2)).not.toThrow(); // both have id=1
    // But the elements are different
    expect(getElement(id1!, ctx1).tagName).toBe("A");
    expect(getElement(id2!, ctx2).tagName).toBe("B");
  });
});

describe("SEMA_IDENT_RE", () => {
  it.each(["foo", "bar/baz", "nil?", "set!", "my-fn"])(
    "matches valid name: %s",
    (name) => {
      expect(SEMA_IDENT_RE.test(name)).toBe(true);
    }
  );

  it.each(["", "123", "(foo)", "a b"])(
    "rejects invalid name: %s",
    (name) => {
      expect(SEMA_IDENT_RE.test(name)).toBe(false);
    }
  );
});
