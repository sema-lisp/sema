/**
 * @sema-lang/sema-web — Sema as a web scripting language
 *
 * Embed Sema in web pages with DOM bindings, persistent storage,
 * and `<script type="text/sema">` support.
 *
 * ## Quick Start
 *
 * Add to your HTML page:
 *
 * ```html
 * <script type="module">
 *   import { SemaWeb } from "@sema-lang/sema-web";
 *   await SemaWeb.init();
 * </script>
 *
 * <script type="text/sema">
 *   (let ((el (dom/create-element "h1")))
 *     (dom/set-text! el "Hello from Sema!")
 *     (dom/append-child! (dom/query "body") el))
 * </script>
 * ```
 *
 * ## Manual Usage
 *
 * ```js
 * import { SemaWeb } from "@sema-lang/sema-web";
 *
 * const web = await SemaWeb.create();
 * web.eval('(dom/set-text! (dom/query "#app") "Hello!")');
 * ```
 *
 * @module
 */

import { SemaInterpreter } from "@sema-lang/sema";
import type { InterpreterOptions } from "@sema-lang/sema";
import { SemaWebContext, disposeContextResources } from "./context.js";
import { registerDomBindings } from "./dom.js";
import { registerStoreBindings } from "./store.js";
import { registerReactiveBindings } from "./reactive.js";
import { registerSipBindings } from "./sip.js";
import { registerComponentBindings, disposeAllComponents } from "./component.js";
import { registerLlmBindings } from "./llm.js";
import type { LlmProxyOptions } from "./llm.js";
import { registerRouterBindings } from "./router.js";
import { registerCssBindings } from "./css.js";
import { registerHttpBindings } from "./http.js";
import { loadScripts } from "./loader.js";
import type { LoaderOptions } from "./loader.js";

/** Options for SemaWeb initialization. */
export interface SemaWebOptions extends InterpreterOptions {
  /**
   * Whether to auto-discover and evaluate `<script type="text/sema">` tags.
   * Default: `true`.
   */
  autoLoad?: boolean;

  /**
   * Whether to register `dom/*` namespace functions.
   * Default: `true`.
   */
  dom?: boolean;

  /**
   * Whether to register `store/*` namespace functions.
   * Default: `true`.
   */
  store?: boolean;

  /**
   * Whether to register reactive state bindings (`state`, `put!`, `update!`,
   * `deref`, `computed`, `batch`, `watch`).
   * Default: `true`.
   */
  reactive?: boolean;

  /**
   * Whether to register SIP rendering bindings (`sip/*` namespace).
   * Default: `true`.
   */
  sip?: boolean;

  /**
   * Whether to register component/mount bindings (`component/*` namespace
   * plus `mount!` convenience function).
   * Automatically enables `reactive` and `sip` if they are not explicitly disabled.
   * Default: `true`.
   */
  components?: boolean;

  /**
   * Options for the script loader.
   */
  loader?: LoaderOptions;

  /**
   * Whether to register `console/*` namespace functions.
   * Default: `true`.
   */
  console?: boolean;

  /**
   * Whether to register `router/*` namespace functions for SPA routing.
   * Default: `true`.
   */
  router?: boolean;

  /**
   * Whether to register `css/*` namespace functions for scoped CSS.
   * Default: `true`.
   */
  css?: boolean;

  /**
   * Whether to register browser-specific `http/*` functions (SSE, etc.).
   * Default: `true`.
   */
  http?: boolean;

  /**
   * LLM proxy configuration. When provided, registers `llm/*` namespace
   * functions that forward requests to the specified backend proxy URL.
   *
   * The proxy server holds API keys and forwards to LLM providers.
   * This is the secure way to use LLM functions from the browser.
   *
   * Can be a full options object or just the proxy URL string.
   *
   * @example
   * ```js
   * // Simple: just the URL
   * await SemaWeb.create({ llmProxy: "https://api.example.com/llm" });
   *
   * // Full options
   * await SemaWeb.create({
   *   llmProxy: {
   *     url: "https://api.example.com/llm",
   *     token: "user-session-token",
   *     timeout: 30000,
   *   },
   * });
   * ```
   */
  llmProxy?: string | LlmProxyOptions;
}

/** Result of evaluating Sema code. */
export interface EvalResult {
  value: string | null;
  output: string[];
  error: string | null;
}

/**
 * Sema web runtime — wraps SemaInterpreter with browser-specific bindings.
 *
 * Provides:
 * - `dom/*` functions for DOM manipulation
 * - `store/*` functions for localStorage/sessionStorage
 * - `console/*` functions for browser console access
 * - Reactive state with `state`, `put!`, `update!`, `computed`, `batch`, `watch`
 * - SIP declarative rendering (`sip/*` namespace)
 * - Component system with `mount!`, `defcomponent`, `local`, `on-mount`
 * - `router/*` hash-based SPA routing
 * - `css/*` scoped CSS injection
 * - `http/*` browser-specific HTTP (SSE)
 * - Auto-loading of `<script type="text/sema">` tags
 *
 * @example
 * ```js
 * // Auto-init: discovers and runs all <script type="text/sema"> tags
 * await SemaWeb.init();
 *
 * // Manual: create instance and evaluate code
 * const web = await SemaWeb.create({ autoLoad: false });
 * web.eval('(dom/set-text! (dom/query "#greeting") "Hello!")');
 * ```
 */
export class SemaWeb {
  private _interp: SemaInterpreter;
  private _ctx: SemaWebContext;

  private constructor(interp: SemaInterpreter, ctx: SemaWebContext) {
    this._interp = interp;
    this._ctx = ctx;
  }

  /**
   * Create a SemaWeb instance with browser bindings registered.
   *
   * @param opts - Configuration options
   * @returns A ready-to-use SemaWeb instance
   */
  static async create(opts?: SemaWebOptions): Promise<SemaWeb> {
    const interp = await SemaInterpreter.create(opts);
    const ctx = new SemaWebContext();
    const web = new SemaWeb(interp, ctx);

    // Register browser bindings
    if (opts?.dom !== false) {
      registerDomBindings(interp, ctx);
    }

    if (opts?.store !== false) {
      registerStoreBindings(interp, ctx);
    }

    if (opts?.console !== false) {
      registerConsoleBindings(interp);
    }

    // Reactive bindings (state, put!, update!, computed, batch, watch)
    // Auto-enabled if components are enabled
    if (opts?.reactive !== false || opts?.components !== false) {
      registerReactiveBindings(interp, ctx);
    }

    // SIP rendering (declarative DOM from vectors/maps)
    // Auto-enabled if components are enabled
    if (opts?.sip !== false || opts?.components !== false) {
      registerSipBindings(interp, ctx);
    }

    // Component system (mount!/unmount! with reactive re-rendering)
    if (opts?.components !== false) {
      registerComponentBindings(interp, ctx);
    }

    // Router bindings (hash-based SPA routing)
    if (opts?.router !== false) {
      registerRouterBindings(interp, ctx);
    }

    // CSS bindings (scoped style injection)
    if (opts?.css !== false) {
      registerCssBindings(interp, ctx);
    }

    // HTTP bindings (SSE, browser-specific wrappers)
    if (opts?.http !== false) {
      registerHttpBindings(interp, ctx);
    }

    // LLM proxy bindings (forward llm/* calls to backend server)
    if (opts?.llmProxy) {
      const proxyOpts: LlmProxyOptions =
        typeof opts.llmProxy === "string"
          ? { url: opts.llmProxy }
          : opts.llmProxy;
      registerLlmBindings(interp, proxyOpts, ctx);
    }

    // Auto-discover and evaluate <script type="text/sema"> tags
    if (opts?.autoLoad !== false) {
      await loadScripts(interp, opts?.loader);
    }

    return web;
  }

  /**
   * Convenience: create a SemaWeb instance with default options and auto-load scripts.
   *
   * Equivalent to `SemaWeb.create()` — discovers and evaluates all
   * `<script type="text/sema">` tags in the document.
   *
   * @param opts - Configuration options
   * @returns A ready-to-use SemaWeb instance
   */
  static async init(opts?: SemaWebOptions): Promise<SemaWeb> {
    return SemaWeb.create(opts);
  }

  /**
   * Evaluate a string of Sema code with browser bindings available.
   *
   * @param code - Sema source code
   * @returns The evaluation result
   */
  eval(code: string): EvalResult {
    return this._interp.evalStr(code);
  }

  /**
   * Evaluate Sema code with async HTTP support.
   *
   * @param code - Sema source code
   * @returns The evaluation result
   */
  async evalAsync(code: string): Promise<EvalResult> {
    return this._interp.evalStrAsync(code);
  }

  /**
   * Register a JavaScript function callable from Sema code.
   *
   * @param name - Function name in Sema
   * @param fn - JavaScript function
   */
  registerFunction(name: string, fn: (...args: any[]) => any): void {
    this._interp.registerFunction(name, fn);
  }

  /**
   * Preload a Sema module so that `(import "name")` works.
   *
   * @param name - Module name
   * @param source - Sema source code
   */
  preloadModule(name: string, source: string): void {
    this._interp.preloadModule(name, source);
  }

  /**
   * Get the underlying SemaInterpreter instance.
   *
   * Useful for advanced operations like VFS access.
   */
  get interpreter(): SemaInterpreter {
    return this._interp;
  }

  /**
   * Get the SemaWebContext instance.
   *
   * Useful for advanced operations requiring direct context access.
   */
  get context(): SemaWebContext {
    return this._ctx;
  }

  /**
   * Get the Sema interpreter version.
   */
  version(): string {
    return this._interp.version();
  }

  /**
   * Free the interpreter's WASM memory.
   * The instance cannot be used after calling this method.
   */
  dispose(): void {
    disposeAllComponents(this._ctx);
    disposeContextResources(this._ctx);
    this._interp.dispose();
  }
}

/**
 * Register `console/*` namespace functions.
 */
function registerConsoleBindings(interp: SemaInterpreter): void {
  interp.registerFunction("console/log", (...args: any[]) => {
    console.log(...args);
    return null;
  });

  interp.registerFunction("console/warn", (...args: any[]) => {
    console.warn(...args);
    return null;
  });

  interp.registerFunction("console/error", (...args: any[]) => {
    console.error(...args);
    return null;
  });

  interp.registerFunction("console/info", (...args: any[]) => {
    console.info(...args);
    return null;
  });

  interp.registerFunction("console/debug", (...args: any[]) => {
    console.debug(...args);
    return null;
  });

  interp.registerFunction("console/clear", () => {
    console.clear();
    return null;
  });

  interp.registerFunction("console/time", (label: string) => {
    console.time(label);
    return null;
  });

  interp.registerFunction("console/time-end", (label: string) => {
    console.timeEnd(label);
    return null;
  });
}

// Re-export types
export type { LoaderOptions } from "./loader.js";
export type { LlmProxyOptions } from "./llm.js";
export { SemaWebContext } from "./context.js";
export type { MountedComponent, ErrorHandler } from "./context.js";
export { registerDomBindings } from "./dom.js";
export { registerStoreBindings } from "./store.js";
export { registerReactiveBindings } from "./reactive.js";
export { registerSipBindings, renderSip } from "./sip.js";
export { registerComponentBindings } from "./component.js";
export { registerLlmBindings } from "./llm.js";
export { registerRouterBindings } from "./router.js";
export { registerCssBindings } from "./css.js";
export { registerHttpBindings } from "./http.js";
export { loadScripts } from "./loader.js";
