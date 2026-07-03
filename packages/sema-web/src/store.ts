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
  // --- localStorage ---

  interp.registerFunction("store/get", (key: string) => {
    try {
      const val = localStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/get:${key}`);
      return null;
    }
  });

  interp.registerFunction("store/set!", (key: string, value: any) => {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/set!:${key}`);
    }
    return null;
  });

  interp.registerFunction("store/remove!", (key: string) => {
    localStorage.removeItem(key);
    return null;
  });

  interp.registerFunction("store/clear!", () => {
    localStorage.clear();
    return null;
  });

  interp.registerFunction("store/keys", () => {
    const keys: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key !== null) keys.push(key);
    }
    return keys;
  });

  interp.registerFunction("store/has?", (key: string) => {
    return localStorage.getItem(key) !== null;
  });

  // --- sessionStorage ---

  interp.registerFunction("store/session-get", (key: string) => {
    try {
      const val = sessionStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/session-get:${key}`);
      return null;
    }
  });

  interp.registerFunction("store/session-set!", (key: string, value: any) => {
    try {
      sessionStorage.setItem(key, JSON.stringify(value));
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/session-set!:${key}`);
    }
    return null;
  });

  interp.registerFunction("store/session-remove!", (key: string) => {
    sessionStorage.removeItem(key);
    return null;
  });

  interp.registerFunction("store/session-clear!", () => {
    sessionStorage.clear();
    return null;
  });
}
