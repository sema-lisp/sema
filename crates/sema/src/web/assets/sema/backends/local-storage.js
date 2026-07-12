import { WebStorageBackend } from "./web-storage.js";
/**
 * VFS backend that persists files to localStorage.
 *
 * Simple and synchronous — good for small projects (&lt; 5 MB).
 * localStorage has a ~5–10 MB limit per origin in most browsers.
 *
 * @example
 * ```ts
 * const sema = await SemaInterpreter.create({
 *   vfs: new LocalStorageBackend({ namespace: "my-project" }),
 * });
 * ```
 */
export class LocalStorageBackend extends WebStorageBackend {
    constructor(opts) {
        super(localStorage, opts);
    }
}
