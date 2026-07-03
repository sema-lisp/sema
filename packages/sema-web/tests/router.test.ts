import { beforeEach, describe, expect, it, vi } from "vitest";
import { registerRouterBindings } from "../src/router.js";
import { SemaWebContext } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";

describe("registerRouterBindings", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    window.location.hash = "#/";
    registerRouterBindings(interp, ctx);
  });

  it("matches literal route patterns with regex metacharacters", () => {
    interp.getFunction("router/init!")!({
      "/notes/v1.0+draft?": "notes-page",
    });

    interp.getFunction("router/replace!")!("/notes/v1.0+draft?");

    const routeSignalId = interp.getFunction("router/current")!();
    const routeSignal = ctx.signals.get(routeSignalId);
    expect(routeSignal?.value).toEqual({
      path: "/notes/v1.0+draft?",
      params: {},
      handler: "notes-page",
    });
  });

  it("decodes route params from the URL hash", () => {
    interp.getFunction("router/init!")!({
      "/todos/:id": "todo-detail",
    });

    interp.getFunction("router/replace!")!("/todos/hello%20world%2F42");

    const routeSignalId = interp.getFunction("router/current")!();
    const routeSignal = ctx.signals.get(routeSignalId);
    expect(routeSignal?.value).toEqual({
      path: "/todos/hello%20world%2F42",
      params: { id: "hello world/42" },
      handler: "todo-detail",
    });
  });

  it("keeps malformed percent escapes as the original route parameter", () => {
    interp.getFunction("router/init!")!({
      "/search/:term": "search-page",
    });

    interp.getFunction("router/replace!")!("/search/%E0%A4%A");

    const routeSignalId = interp.getFunction("router/current")!();
    expect(ctx.signals.get(routeSignalId)?.value).toEqual({
      path: "/search/%E0%A4%A",
      params: { term: "%E0%A4%A" },
      handler: "search-page",
    });
  });

  it("strips keyword-style route patterns and handlers from Sema maps", () => {
    interp.getFunction("router/init!")!({
      ":/settings": ":settings-page",
    });

    interp.getFunction("router/replace!")!("/settings");

    const routeSignalId = interp.getFunction("router/current")!();
    expect(ctx.signals.get(routeSignalId)?.value).toMatchObject({
      path: "/settings",
      params: {},
      handler: "settings-page",
    });
  });

  it("updates current route on hashchange and returns null when no route matches", () => {
    interp.getFunction("router/init!")!({
      "/known": "known-page",
    });

    window.location.hash = "#/known";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    const routeSignalId = interp.getFunction("router/current")!();
    expect(ctx.signals.get(routeSignalId)?.value).toMatchObject({ handler: "known-page" });

    window.location.hash = "#/unknown";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    expect(ctx.signals.get(routeSignalId)?.value).toBeNull();
  });

  it("reinitializing routes removes the previous hashchange listener from cleanup hooks", () => {
    interp.getFunction("router/init!")!({ "/a": "a-page" });
    const firstCleanup = [...ctx.cleanupHooks][0];
    const removeSpy = vi.spyOn(window, "removeEventListener");

    interp.getFunction("router/init!")!({ "/b": "b-page" });

    expect(removeSpy).toHaveBeenCalledWith("hashchange", expect.any(Function));
    expect(ctx.cleanupHooks.has(firstCleanup!)).toBe(false);
    expect(ctx.cleanupHooks.size).toBe(1);
  });

  it("router/push! and router/back! delegate to browser history APIs", () => {
    const backSpy = vi.spyOn(window.history, "back").mockImplementation(() => {});

    interp.getFunction("router/push!")!("/next");
    expect(window.location.hash).toBe("#/next");

    interp.getFunction("router/back!")!();
    expect(backSpy).toHaveBeenCalledOnce();
  });

  it("throws a clear setup error when Sema wrapper registration fails", () => {
    const badInterp = createMockInterpreter();
    badInterp.evalStr = () => ({ value: null, output: [], error: "wrapper boom" });

    expect(() => registerRouterBindings(badInterp, new SemaWebContext())).toThrow(/wrapper boom/);
  });
});
