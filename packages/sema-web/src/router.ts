/**
 * Hash-based SPA router for Sema Web — built on signals.
 *
 * Provides `router/*` namespace functions for declaring routes,
 * navigating, and reading the current route as reactive state.
 *
 * ## Usage
 *
 * ```sema
 * (router/init! {"/todos" "todo-page"
 *                "/todos/:id" "todo-detail"
 *                "/settings" "settings-page"})
 *
 * (router/push! "/todos/42")
 * (router/current-route)  ;; => {:path "/todos/42" :params {:id "42"} :handler "todo-detail"}
 * ```
 *
 * @module
 */

import { signal } from "@preact/signals-core";
import type { SemaWebContext } from "./context.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
  evalStr(code: string): { value: string | null; output: string[]; error: string | null };
}

interface Route {
  pattern: string;
  regex: RegExp;
  paramNames: string[];
  handler: string;
}

interface RouteMatch {
  path: string;
  params: Record<string, string>;
  handler: string;
}

function escapeRegexLiteral(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function decodeRouteParam(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

/**
 * Compile a route pattern (e.g., "/todos/:id") into a regex and param name list.
 */
function compileRoute(pattern: string): { regex: RegExp; paramNames: string[] } {
  const paramNames: string[] = [];
  let regexStr = "^";
  let lastIndex = 0;

  pattern.replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, (match, name, offset) => {
    regexStr += escapeRegexLiteral(pattern.slice(lastIndex, offset));
    paramNames.push(name);
    regexStr += "([^/]+)";
    lastIndex = offset + match.length;
    return match;
  });

  regexStr += escapeRegexLiteral(pattern.slice(lastIndex));
  regexStr += "$";

  return { regex: new RegExp(regexStr), paramNames };
}

/**
 * Register `router/*` namespace functions.
 *
 * Functions registered:
 * - `router/init!` — register routes from a map of pattern -> handler name
 * - `router/push!` — navigate to a path (sets location.hash)
 * - `router/replace!` — replace current route without adding history entry
 * - `router/back!` — go back in history
 * - `router/current` — returns the signal ID for the current route match
 *
 * Sema wrapper:
 * - `(router/current-route)` — convenience: dereferences the route signal
 */
export function registerRouterBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {
  const routes: Route[] = [];
  let removeHashChangeListener: (() => void) | null = null;

  // Create a signal for the current route match
  const routeSignalId = ctx.nextSignalId++;
  const routeSignal = signal<RouteMatch | null>(null);
  ctx.signals.set(routeSignalId, routeSignal as any);

  function matchRoute(path: string): RouteMatch | null {
    for (const route of routes) {
      const match = path.match(route.regex);
      if (match) {
        const params: Record<string, string> = {};
        route.paramNames.forEach((name, i) => {
          params[name] = decodeRouteParam(match[i + 1]);
        });
        return { path, params, handler: route.handler };
      }
    }
    return null;
  }

  function updateRoute(): void {
    const hash = window.location.hash.slice(1) || "/";
    routeSignal.value = matchRoute(hash);
  }

  // router/init! — register routes from a map
  interp.registerFunction("router/init!", (routeMap: Record<string, string>) => {
    routes.length = 0;
    for (let [pattern, handler] of Object.entries(routeMap)) {
      // Sema keywords come through with leading colon — strip if present
      if (pattern.startsWith(":")) pattern = pattern.slice(1);
      if (typeof handler === "string" && handler.startsWith(":")) handler = handler.slice(1);

      const { regex, paramNames } = compileRoute(String(pattern));
      routes.push({ pattern: String(pattern), regex, paramNames, handler: String(handler) });
    }
    if (removeHashChangeListener) {
      removeHashChangeListener();
      ctx.cleanupHooks.delete(removeHashChangeListener);
    }
    window.addEventListener("hashchange", updateRoute);
    removeHashChangeListener = () => {
      window.removeEventListener("hashchange", updateRoute);
    };
    ctx.cleanupHooks.add(removeHashChangeListener);
    updateRoute();
    return null;
  });

  // router/push! — navigate by setting location.hash
  interp.registerFunction("router/push!", (path: string) => {
    window.location.hash = path;
    return null;
  });

  // router/replace! — replace current route without history entry
  interp.registerFunction("router/replace!", (path: string) => {
    window.history.replaceState(null, "", `#${path}`);
    updateRoute();
    return null;
  });

  // router/back! — go back in history
  interp.registerFunction("router/back!", () => {
    window.history.back();
    return null;
  });

  // router/current — returns the route signal ID (for use with deref)
  interp.registerFunction("router/current", () => routeSignalId);

  // --- Sema-side convenience wrapper ---
  const semaResult = interp.evalStr(`
    (define (router/current-route) (deref (router/current)))
  `);

  if (semaResult.error) {
    throw new Error(`[sema-web] Failed to register router wrappers: ${semaResult.error}`);
  }
}
