/**
 * @sema-lang/sema — Sema Lisp interpreter for JavaScript
 *
 * A client-side scripting engine powered by WebAssembly.
 * Embed Sema as a scripting language in your web applications.
 *
 * @example
 * ```js
 * import { SemaInterpreter } from "@sema-lang/sema";
 *
 * const sema = await SemaInterpreter.create();
 * const result = sema.evalStr("(+ 1 2 3)");
 * console.log(result.value); // "6"
 * ```
 */

import type { VFSBackend, VFSHost } from "./vfs.js";

// NOTE: When published, @sema-lang/sema-wasm provides these.
// For local dev, the wasm-pack output is at ../../playground/pkg/ or ../../crates/sema-wasm/pkg/
// The import path is resolved at runtime via the wasmUrl option or default init.

/** Result of evaluating Sema code. */
export interface EvalResult {
  /** The string representation of the result value, or null if the expression returned nil. */
  value: string | null;
  /** Lines printed to stdout during evaluation (via `print`, `println`, `display`). */
  output: string[];
  /** Error message if evaluation failed, or null on success. */
  error: string | null;
}

/** Options for creating a SemaInterpreter. */
export interface InterpreterOptions {
  /**
   * URL to the `.wasm` binary. Required when loading from a CDN or
   * when the default resolution doesn't work.
   *
   * @example
   * ```js
   * const sema = await SemaInterpreter.create({
   *   wasmUrl: "https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm@1.9.0/sema_wasm_bg.wasm"
   * });
   * ```
   */
  wasmUrl?: string | URL;

  /**
   * Whether to include the standard library. Default: `true`.
   * Set to `false` for a minimal interpreter with only special forms.
   */
  stdlib?: boolean;

  /**
   * Array of capabilities to deny. Available capabilities:
   * - `"network"` — deny HTTP functions (http/get, http/post, etc.)
   * - `"fs-read"` — deny VFS read operations
   * - `"fs-write"` — deny VFS write operations
   *
   * @example
   * ```js
   * const sema = await SemaInterpreter.create({ deny: ["network"] });
   * ```
   */
  deny?: Array<"network" | "fs-read" | "fs-write">;

  /**
   * Optional VFS backend for persisting files across page reloads.
   *
   * Built-in backends:
   * - {@link MemoryBackend} — ephemeral, no persistence (default behavior)
   * - {@link LocalStorageBackend} — persist to localStorage (~5 MB limit)
   * - {@link SessionStorageBackend} — persist within the tab session
   * - {@link IndexedDBBackend} — persist to IndexedDB (recommended for production)
   *
   * @example
   * ```js
   * import { SemaInterpreter, IndexedDBBackend } from "@sema-lang/sema";
   * const sema = await SemaInterpreter.create({
   *   vfs: new IndexedDBBackend({ namespace: "my-project" }),
   * });
   * ```
   */
  vfs?: VFSBackend;
}

/** Virtual filesystem usage statistics. */
export interface VFSStats {
  /** Number of files currently in the VFS. */
  files: number;
  /** Total bytes used. */
  bytes: number;
  /** Maximum number of files allowed. */
  maxFiles: number;
  /** Maximum total bytes allowed. */
  maxBytes: number;
  /** Maximum bytes per individual file. */
  maxFileBytes: number;
}

/** Result of loading a compiled Sema web archive. */
export interface ArchiveLoadResult {
  ok: boolean;
  entryPoint: string | null;
  fileCount: number;
  semaVersion: string | null;
  buildTarget: string | null;
  buildTimestamp: string | null;
  error: string | null;
}

export interface SemaCallback {
  (...args: any[]): any;
  __semaCallbackHandle?: number;
  __semaRelease?: () => void;
}

interface SemaCallbackMarker {
  __semaCallbackHandle: number;
}

// Internal state
let _init: typeof import("@sema-lang/sema-wasm").default | null = null;
let _SemaInterpreter: typeof import("@sema-lang/sema-wasm").SemaInterpreter | null = null;
let _initialized = false;
let _initPromise: Promise<void> | null = null;

async function ensureInit(wasmUrl?: string | URL): Promise<void> {
  if (_initialized) return;
  if (_initPromise) {
    await _initPromise;
    return;
  }

  _initPromise = (async () => {
    // Dynamic import so bundlers can tree-shake when not used
    const mod = await import("@sema-lang/sema-wasm");
    _init = mod.default;
    _SemaInterpreter = mod.SemaInterpreter;

    if (wasmUrl) {
      await _init!(wasmUrl as any);
    } else {
      await _init!();
    }
    _initialized = true;
  })();

  await _initPromise;
}

/**
 * A Sema Lisp interpreter instance.
 *
 * Each interpreter has its own isolated environment — variables defined in one
 * interpreter are not visible in another.
 *
 * @example
 * ```js
 * const sema = await SemaInterpreter.create();
 *
 * // Evaluate expressions
 * sema.evalStr("(define x 42)");
 * const r = sema.evalStr("(* x x)");
 * console.log(r.value); // "1764"
 *
 * // Register JS functions
 * sema.registerFunction("greet", (name) => `Hello, ${name}!`);
 * sema.evalStr('(greet "world")'); // => "Hello, world!"
 *
 * // Preload modules
 * sema.preloadModule("utils", "(define (double x) (* x 2))");
 * sema.evalStr('(import "utils")');
 * sema.evalStr("(double 21)"); // => "42"
 * ```
 */
export class SemaInterpreter {
  private _inner: any; // SemaInterpreter wasm instance
  private _vfsBackend: VFSBackend | null;
  private _callbackWrappers = new Map<number, SemaCallback>();

  private constructor(inner: any, vfsBackend: VFSBackend | null = null) {
    this._inner = inner;
    this._vfsBackend = vfsBackend;
  }

  private _extractCallbackHandle(value: any): number | null {
    if (!value || (typeof value !== "object" && typeof value !== "function")) {
      return null;
    }
    const handle = (value as SemaCallbackMarker).__semaCallbackHandle;
    return typeof handle === "number" ? handle : null;
  }

  private _wrapCallbackHandle(handle: number): SemaCallback {
    const existing = this._callbackWrappers.get(handle);
    if (existing) return existing;

    const wrapped = ((...args: any[]) => {
      const result = this._inner.invokeCallback(handle, args);
      return this._deserializeValue(result);
    }) as SemaCallback;

    Object.defineProperty(wrapped, "__semaCallbackHandle", {
      configurable: true,
      enumerable: false,
      value: handle,
      writable: false,
    });

    Object.defineProperty(wrapped, "__semaRelease", {
      configurable: true,
      enumerable: false,
      value: () => {
        this._callbackWrappers.delete(handle);
        this._inner.releaseCallback(handle);
      },
      writable: false,
    });

    this._callbackWrappers.set(handle, wrapped);
    return wrapped;
  }

  private _deserializeValue(value: any): any {
    const handle = this._extractCallbackHandle(value);
    if (handle != null) {
      return this._wrapCallbackHandle(handle);
    }
    return value;
  }

  /**
   * Create a new SemaInterpreter instance.
   *
   * This initializes the WASM module (once, globally) and creates an interpreter.
   * The WASM module is cached — subsequent calls are fast.
   *
   * @param opts - Configuration options
   * @returns A ready-to-use SemaInterpreter
   *
   * @example
   * ```js
   * // Default: full standard library
   * const sema = await SemaInterpreter.create();
   *
   * // Minimal: only special forms
   * const minimal = await SemaInterpreter.create({ stdlib: false });
   *
   * // From CDN
   * const cdn = await SemaInterpreter.create({
   *   wasmUrl: "https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm@1.9.0/sema_wasm_bg.wasm"
   * });
   * ```
   */
  static async create(opts?: InterpreterOptions): Promise<SemaInterpreter> {
    await ensureInit(opts?.wasmUrl);

    const needsOptions = opts?.stdlib === false || (opts?.deny && opts.deny.length > 0);
    let inner: any;

    if (needsOptions) {
      inner = _SemaInterpreter!.createWithOptions({
        stdlib: opts?.stdlib ?? true,
        deny: opts?.deny,
      });
    } else {
      inner = new _SemaInterpreter!();
    }

    const interp = new SemaInterpreter(inner, opts?.vfs ?? null);

    if (interp._vfsBackend) {
      await interp._vfsBackend.init?.();
      await interp._vfsBackend.hydrate(interp._vfsHost());
    }

    return interp;
  }

  private _vfsHost(): VFSHost {
    return {
      readFile: (p) => this.readFile(p),
      writeFile: (p, c) => this.writeFile(p, c),
      deleteFile: (p) => this.deleteFile(p),
      mkdir: (p) => this.mkdir(p),
      listFiles: (d) => this.listFiles(d),
      fileExists: (p) => this.fileExists(p),
      isDirectory: (p) => this.isDirectory(p),
      resetVFS: () => this.resetVFS(),
    };
  }

  /**
   * Evaluate a string of Sema code.
   *
   * Definitions persist across calls — you can `define` a function in one call
   * and use it in the next.
   *
   * @param code - Sema source code to evaluate
   * @returns The evaluation result
   *
   * @example
   * ```js
   * const r = sema.evalStr("(map (lambda (x) (* x x)) '(1 2 3 4 5))");
   * console.log(r.value);  // "(1 4 9 16 25)"
   * console.log(r.output); // [] (no print statements)
   * console.log(r.error);  // null (no error)
   * ```
   */
  evalStr(code: string): EvalResult {
    return this._inner.evalGlobal(code) as EvalResult;
  }

  /**
   * Evaluate code with async HTTP support.
   *
   * Use this when your Sema code uses `http/get` or other network functions.
   * The interpreter will automatically handle the async fetch operations.
   *
   * @param code - Sema source code to evaluate
   * @returns The evaluation result
   *
   * @example
   * ```js
   * const r = await sema.evalStrAsync('(http/get "https://api.example.com/data")');
   * ```
   */
  async evalStrAsync(code: string): Promise<EvalResult> {
    return (await this._inner.evalAsync(code)) as EvalResult;
  }

  /**
   * Load a compiled `.vfs` archive produced by `sema build --target web`.
   *
   * The archive's modules become available to `runEntry()` and `import`.
   */
  loadArchive(bytes: ArrayBuffer | Uint8Array): ArchiveLoadResult {
    const result = this._inner.loadArchive(toUint8Array(bytes)) as ArchiveLoadResult;
    if (!result.ok) {
      throw new Error(result.error ?? "Failed to load archive");
    }
    return result;
  }

  /**
   * Execute an embedded archive entry path synchronously.
   */
  runEntry(path: string): EvalResult {
    return this._inner.runEntry(path) as EvalResult;
  }

  /**
   * Execute an embedded archive entry path with async HTTP replay support.
   */
  async runEntryAsync(path: string): Promise<EvalResult> {
    return (await this._inner.runEntryAsync(path)) as EvalResult;
  }

  /**
   * Invoke a named global function directly with native JS arguments.
   */
  invokeGlobal(name: string, ...args: any[]): any {
    return this._deserializeValue(this._inner.invokeGlobal(name, args));
  }

  /**
   * Invoke a callback handle or wrapped Sema callback function.
   */
  invokeCallback(callback: number | SemaCallback, ...args: any[]): any {
    const handle =
      typeof callback === "number"
        ? callback
        : this._extractCallbackHandle(callback);
    if (handle == null) {
      throw new Error("Expected a Sema callback handle or wrapped callback function");
    }
    return this._deserializeValue(this._inner.invokeCallback(handle, args));
  }

  /**
   * Release a callback handle created when a Sema function value crossed into JS.
   */
  releaseCallback(callback: number | SemaCallback): void {
    const handle =
      typeof callback === "number"
        ? callback
        : this._extractCallbackHandle(callback);
    if (handle == null) return;
    this._callbackWrappers.delete(handle);
    this._inner.releaseCallback(handle);
  }

  /**
   * Register a JavaScript function that can be called from Sema code.
   *
   * Arguments are passed as native JavaScript values (numbers, strings,
   * booleans, null, arrays, objects). The return value is converted back
   * to a Sema value.
   *
   * @param name - The function name in Sema (e.g., "my-fn")
   * @param fn - The JavaScript function to call
   *
   * @example
   * ```js
   * // Simple function — args are native JS values
   * sema.registerFunction("add1", (n) => n + 1);
   *
   * // Multiple args
   * sema.registerFunction("greet", (greeting, name) => `${greeting}, ${name}!`);
   *
   * // Returning structured data (objects become Sema maps)
   * sema.registerFunction("get-user", (id) => ({ name: "Alice", age: 30 }));
   * ```
   */
  registerFunction(name: string, fn: (...args: any[]) => any): void {
    this._inner.registerFunction(name, (...args: any[]) => {
      const deserializedArgs = args.map((arg) => this._deserializeValue(arg));
      return fn(...deserializedArgs);
    });
  }

  /**
   * Preload a virtual module so that `(import "name")` works without a file.
   *
   * @param name - The module name (used in `(import "name")`)
   * @param source - Sema source code defining the module's exports
   * @throws If the module source has syntax or evaluation errors
   *
   * @example
   * ```js
   * sema.preloadModule("utils", `
   *   (define (double x) (* x 2))
   *   (define pi 3.14159)
   * `);
   *
   * sema.evalStr('(import "utils")');
   * sema.evalStr("(double pi)"); // => "6.28318"
   * ```
   */
  preloadModule(name: string, source: string): void {
    const result = this._inner.preloadModule(name, source) as { ok: boolean; error: string | null };
    if (!result.ok) {
      throw new Error(`Failed to preload module "${name}": ${result.error}`);
    }
  }

  /**
   * Read a file from the virtual filesystem.
   * @returns The file contents, or null if the file doesn't exist.
   */
  readFile(path: string): string | null {
    const result = this._inner.readFile(path);
    return result === null || result === undefined ? null : result;
  }

  /**
   * Write a file to the virtual filesystem.
   * Quotas: 1 MB per file, 16 MB total, 256 files max.
   * @throws If a VFS quota is exceeded.
   */
  writeFile(path: string, content: string): void {
    const error = this._inner.writeFile(path, content);
    if (typeof error === "string") {
      throw new Error(error);
    }
  }

  /**
   * Delete a file from the virtual filesystem.
   * @returns true if the file existed and was deleted.
   */
  deleteFile(path: string): boolean {
    return this._inner.deleteFile(path);
  }

  /**
   * List entries in a VFS directory.
   * @returns Array of file and directory names (not full paths).
   */
  listFiles(dir?: string): string[] {
    return Array.from(this._inner.listFiles(dir ?? "/"));
  }

  /**
   * Check if a path exists in the VFS (file or directory).
   */
  fileExists(path: string): boolean {
    return this._inner.fileExists(path);
  }

  /**
   * Create a directory (and parent directories) in the VFS.
   */
  mkdir(path: string): void {
    this._inner.mkdir(path);
  }

  /**
   * Check if a path is a directory in the VFS.
   */
  isDirectory(path: string): boolean {
    return this._inner.isDirectory(path);
  }

  /**
   * Get VFS usage statistics (file count, bytes used, quotas).
   */
  vfsStats(): VFSStats {
    return this._inner.vfsStats() as VFSStats;
  }

  /**
   * Clear all files and directories from the VFS.
   */
  resetVFS(): void {
    this._inner.resetVFS();
  }

  /**
   * Persist VFS changes to the configured backend.
   *
   * No-op if no VFS backend was provided during creation.
   * Call this after eval to save files.
   *
   * @example
   * ```js
   * await sema.evalStrAsync(code);
   * await sema.flushVFS(); // persist to localStorage/IndexedDB/etc.
   * ```
   */
  async flushVFS(): Promise<void> {
    if (this._vfsBackend) {
      await this._vfsBackend.flush(this._vfsHost());
    }
  }

  /**
   * Clear the VFS and the persistent backend (if configured).
   */
  async resetVFSAndBackend(): Promise<void> {
    this._inner.resetVFS();
    await this._vfsBackend?.reset?.();
  }

  /**
   * Get the Sema interpreter version.
   *
   * @returns Version string (e.g., "1.9.0")
   */
  version(): string {
    return this._inner.version();
  }

  /**
   * Free the interpreter's WASM memory.
   *
   * Call this when you're done with the interpreter to release resources.
   * The interpreter cannot be used after calling this method.
   */
  dispose(): void {
    this._callbackWrappers.clear();
    this._inner.free();
  }
}

export type { VFSBackend, VFSHost } from "./vfs.js";
export { MemoryBackend } from "./backends/memory.js";
export { LocalStorageBackend } from "./backends/local-storage.js";
export type { LocalStorageBackendOptions } from "./backends/local-storage.js";
export { SessionStorageBackend } from "./backends/session-storage.js";
export type { SessionStorageBackendOptions } from "./backends/session-storage.js";
export { IndexedDBBackend } from "./backends/indexed-db.js";
export type { IndexedDBBackendOptions } from "./backends/indexed-db.js";

/** @deprecated Use `SemaInterpreter` instead. */
export { SemaInterpreter as Interpreter };

function toUint8Array(bytes: ArrayBuffer | Uint8Array): Uint8Array {
  return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
}
