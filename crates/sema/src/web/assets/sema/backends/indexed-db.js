/**
 * VFS backend that persists files to IndexedDB.
 *
 * Unlike the localStorage-based backends, IndexedDB supports large blobs and
 * doesn't share its quota with other synchronous storage.  All reads and writes
 * are async, which avoids blocking the main thread.
 *
 * Uses a single object store (`"files"`) keyed by `path`.
 *
 * @example
 * ```ts
 * import { SemaInterpreter, IndexedDBBackend } from "@sema-lang/sema";
 *
 * const sema = await SemaInterpreter.create({
 *   vfs: new IndexedDBBackend({ namespace: "my-project" }),
 * });
 * await sema.evalStrAsync(code);
 * await sema.flushVFS();
 * ```
 */
export class IndexedDBBackend {
    constructor(opts) {
        this.db = null;
        this.dbName = opts?.namespace ?? "sema-vfs";
    }
    /** Open (or create) the IndexedDB database and cache the connection. */
    async init() {
        this.db = await this.openDB();
    }
    /**
     * Populate the in-memory WASM VFS from IndexedDB.
     *
     * Directories are restored first (sorted by depth so parents are created
     * before children), then files.
     */
    async hydrate(host) {
        const db = this.db ?? await this.openDB();
        const records = await this.getAll(db);
        // Restore directories first, shallowest to deepest
        const dirs = records
            .filter((r) => r.isDir)
            .sort((a, b) => a.path.split("/").length - b.path.split("/").length);
        for (const rec of dirs) {
            host.mkdir(rec.path);
        }
        // Restore files
        for (const rec of records) {
            if (!rec.isDir && rec.content !== undefined) {
                host.writeFile(rec.path, rec.content);
            }
        }
    }
    /**
     * Persist the current in-memory VFS state to IndexedDB.
     *
     * Clears the object store and writes all files and directories in a single
     * readwrite transaction.
     */
    async flush(host) {
        const db = this.db ?? await this.openDB();
        const tx = db.transaction("files", "readwrite");
        const store = tx.objectStore("files");
        store.clear();
        // Write directories
        const dirs = this.collectDirs(host, "/");
        for (const dir of dirs) {
            store.put({ path: dir, isDir: true });
        }
        // Write files
        const files = this.collectFiles(host, "/");
        for (const filePath of files) {
            const content = host.readFile(filePath);
            if (content !== null) {
                store.put({
                    path: filePath,
                    content,
                    isDir: false,
                });
            }
        }
        await this.txComplete(tx);
    }
    /** Clear all persisted data from the object store. */
    async reset() {
        const db = this.db ?? await this.openDB();
        const tx = db.transaction("files", "readwrite");
        tx.objectStore("files").clear();
        await this.txComplete(tx);
    }
    // ---------------------------------------------------------------------------
    // Private helpers
    // ---------------------------------------------------------------------------
    /** Open the IndexedDB database, creating the object store if needed. */
    openDB() {
        return new Promise((resolve, reject) => {
            const req = indexedDB.open(this.dbName, 1);
            req.onupgradeneeded = () => {
                const db = req.result;
                if (!db.objectStoreNames.contains("files")) {
                    db.createObjectStore("files", { keyPath: "path" });
                }
            };
            req.onsuccess = () => resolve(req.result);
            req.onerror = () => reject(req.error);
        });
    }
    /** Read all records from the `"files"` object store. */
    getAll(db) {
        return new Promise((resolve, reject) => {
            const tx = db.transaction("files", "readonly");
            const req = tx.objectStore("files").getAll();
            req.onsuccess = () => resolve(req.result);
            req.onerror = () => reject(req.error);
        });
    }
    /** Wait for a transaction to complete. */
    txComplete(tx) {
        return new Promise((resolve, reject) => {
            tx.oncomplete = () => resolve();
            tx.onerror = () => reject(tx.error);
        });
    }
    /** Recursively collect all file paths. */
    collectFiles(host, dir) {
        const result = [];
        const entries = host.listFiles(dir);
        for (const name of entries) {
            const full = dir === "/" ? "/" + name : dir + "/" + name;
            if (host.isDirectory(full)) {
                result.push(...this.collectFiles(host, full));
            }
            else {
                result.push(full);
            }
        }
        return result;
    }
    /** Recursively collect all directory paths. */
    collectDirs(host, dir) {
        const result = [];
        const entries = host.listFiles(dir);
        for (const name of entries) {
            const full = dir === "/" ? "/" + name : dir + "/" + name;
            if (host.isDirectory(full)) {
                result.push(full);
                result.push(...this.collectDirs(host, full));
            }
        }
        return result;
    }
}
