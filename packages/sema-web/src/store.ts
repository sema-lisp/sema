/**
 * Store bindings for Sema — registers `store/*` namespace functions.
 *
 * Provides localStorage and sessionStorage access from Sema code.
 *
 * @module
 */

import type { SemaWebContext } from "./context.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
}

/**
 * Register all `store/*` namespace functions on the given interpreter.
 *
 * Functions registered:
 * - `store/get` — get value from localStorage
 * - `store/set!` — set value in localStorage
 * - `store/remove!` — remove key from localStorage
 * - `store/clear!` — clear all localStorage
 * - `store/keys` — list all localStorage keys
 * - `store/has?` — check if key exists in localStorage
 * - `store/session-get` — get value from sessionStorage
 * - `store/session-set!` — set value in sessionStorage
 * - `store/session-remove!` — remove key from sessionStorage
 * - `store/session-clear!` — clear all sessionStorage
 *
 * Values are always serialized as JSON on set and parsed from JSON on get.
 */
export function registerStoreBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {
  // Storage throws (SecurityError, QuotaExceededError) when the browser blocks
  // it, e.g. private mode or a sandboxed iframe. Every binding reports through
  // `ctx.onerror` and returns `fallback` instead of throwing into Sema code.
  const guarded = <T>(context: string, fallback: T, run: () => T): T => {
    try {
      return run();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), context);
      return fallback;
    }
  };

  // --- localStorage ---

  interp.registerFunction("store/get", (key: string) =>
    guarded(`store/get:${key}`, null, () => {
      const val = localStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    }),
  );

  interp.registerFunction("store/set!", (key: string, value: any) =>
    guarded(`store/set!:${key}`, null, () => {
      localStorage.setItem(key, JSON.stringify(value));
      return null;
    }),
  );

  interp.registerFunction("store/remove!", (key: string) =>
    guarded(`store/remove!:${key}`, null, () => {
      localStorage.removeItem(key);
      return null;
    }),
  );

  interp.registerFunction("store/clear!", () =>
    guarded("store/clear!", null, () => {
      localStorage.clear();
      return null;
    }),
  );

  interp.registerFunction("store/keys", () =>
    guarded("store/keys", [] as string[], () => {
      const keys: string[] = [];
      for (let i = 0; i < localStorage.length; i++) {
        const key = localStorage.key(i);
        if (key !== null) keys.push(key);
      }
      return keys;
    }),
  );

  interp.registerFunction("store/has?", (key: string) =>
    guarded(`store/has?:${key}`, false, () => localStorage.getItem(key) !== null),
  );

  // --- sessionStorage ---

  interp.registerFunction("store/session-get", (key: string) =>
    guarded(`store/session-get:${key}`, null, () => {
      const val = sessionStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    }),
  );

  interp.registerFunction("store/session-set!", (key: string, value: any) =>
    guarded(`store/session-set!:${key}`, null, () => {
      sessionStorage.setItem(key, JSON.stringify(value));
      return null;
    }),
  );

  interp.registerFunction("store/session-remove!", (key: string) =>
    guarded(`store/session-remove!:${key}`, null, () => {
      sessionStorage.removeItem(key);
      return null;
    }),
  );

  interp.registerFunction("store/session-clear!", () =>
    guarded("store/session-clear!", null, () => {
      sessionStorage.clear();
      return null;
    }),
  );
}
