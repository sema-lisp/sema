# VFS Backends Library Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ship all 4 VFS backends (Memory, LocalStorage, SessionStorage, IndexedDB) from `@sema-lang/sema`, with documentation on the website.

**Architecture:** Extract a `WebStorageBackend` base class from the existing `LocalStorageBackend`, add `MemoryBackend`, `SessionStorageBackend`, and `IndexedDBBackend`. Export all from the package entrypoint. Update `embedding-js.md` with a VFS Persistence section and update the npm README.

**Tech Stack:** TypeScript (ES2020), IndexedDB API, Web Storage API

---

### Task 1: Extract WebStorageBackend Base Class

**Files:**
- Create: `packages/sema/src/backends/web-storage.ts`
- Modify: `packages/sema/src/backends/local-storage.ts`

**Step 1: Create the base class**

Create `packages/sema/src/backends/web-storage.ts`:

```ts
import type { VFSBackend, VFSHost } from "../vfs.js";

/** Options for WebStorage-based backends. */
export interface WebStorageBackendOptions {
  /**
   * Namespace prefix for storage keys.
   * Each file is stored as `${namespace}:f:${path}`.
   * Directories are stored in a manifest key `${namespace}:__dirs__`.
   * @default "sema-vfs"
   */
  namespace?: string;
}

/**
 * Base class for backends that persist to a Web Storage API
 * (localStorage or sessionStorage).
 *
 * Not exported from the package — use {@link LocalStorageBackend}
 * or {@link SessionStorageBackend} instead.
 */
export class WebStorageBackend implements VFSBackend {
  private storage: Storage;
  private filePrefix: string;
  private dirsKey: string;

  constructor(storage: Storage, opts?: WebStorageBackendOptions) {
    this.storage = storage;
    const ns = opts?.namespace ?? "sema-vfs";
    this.filePrefix = ns + ":f:";
    this.dirsKey = ns + ":__dirs__";
  }

  async hydrate(host: VFSHost): Promise<void> {
    const dirsJson = this.storage.getItem(this.dirsKey);
    if (dirsJson) {
      try {
        const dirs: string[] = JSON.parse(dirsJson);
        for (const dir of dirs) {
          host.mkdir(dir);
        }
      } catch { /* ignore corrupt data */ }
    }

    for (let i = 0; i < this.storage.length; i++) {
      const key = this.storage.key(i);
      if (key && key.startsWith(this.filePrefix)) {
        const path = key.slice(this.filePrefix.length);
        const content = this.storage.getItem(key);
        if (content !== null) {
          host.writeFile(path, content);
        }
      }
    }
  }

  async flush(host: VFSHost): Promise<void> {
    const toRemove: string[] = [];
    for (let i = 0; i < this.storage.length; i++) {
      const key = this.storage.key(i);
      if (key && (key.startsWith(this.filePrefix) || key === this.dirsKey)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) {
      this.storage.removeItem(key);
    }

    const allFiles = this.collectFiles(host, "/");
    for (const path of allFiles) {
      const content = host.readFile(path);
      if (content !== null) {
        this.storage.setItem(this.filePrefix + path, content);
      }
    }

    const dirs = this.collectDirs(host, "/");
    this.storage.setItem(this.dirsKey, JSON.stringify(dirs));
  }

  async reset(): Promise<void> {
    const toRemove: string[] = [];
    for (let i = 0; i < this.storage.length; i++) {
      const key = this.storage.key(i);
      if (key && (key.startsWith(this.filePrefix) || key === this.dirsKey)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) {
      this.storage.removeItem(key);
    }
  }

  /** Recursively collect all file paths. */
  private collectFiles(host: VFSHost, dir: string): string[] {
    const result: string[] = [];
    const entries = host.listFiles(dir);
    for (const name of entries) {
      const full = dir === "/" ? "/" + name : dir + "/" + name;
      if (host.isDirectory(full)) {
        result.push(...this.collectFiles(host, full));
      } else {
        result.push(full);
      }
    }
    return result;
  }

  /** Recursively collect all directory paths. */
  private collectDirs(host: VFSHost, dir: string): string[] {
    const result: string[] = [];
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
```

**Step 2: Rewrite LocalStorageBackend as a thin subclass**

Replace `packages/sema/src/backends/local-storage.ts` with:

```ts
import type { VFSBackend } from "../vfs.js";
import { WebStorageBackend, type WebStorageBackendOptions } from "./web-storage.js";

export type LocalStorageBackendOptions = WebStorageBackendOptions;

/**
 * VFS backend that persists files to localStorage.
 *
 * Simple and synchronous — good for small projects (< 5 MB).
 * localStorage has a ~5–10 MB limit per origin in most browsers.
 *
 * @example
 * ```ts
 * import { SemaInterpreter, LocalStorageBackend } from "@sema-lang/sema";
 *
 * const sema = await SemaInterpreter.create({
 *   vfs: new LocalStorageBackend({ namespace: "my-project" }),
 * });
 * ```
 */
export class LocalStorageBackend extends WebStorageBackend implements VFSBackend {
  constructor(opts?: LocalStorageBackendOptions) {
    super(localStorage, opts);
  }
}
```

**Step 3: Verify build**

Run: `cd packages/sema && npx tsc --noEmit`
Expected: No errors

**Step 4: Commit**

```bash
git add packages/sema/src/backends/web-storage.ts packages/sema/src/backends/local-storage.ts
git commit -m "refactor: extract WebStorageBackend base class from LocalStorageBackend"
```

---

### Task 2: Add MemoryBackend

**Files:**
- Create: `packages/sema/src/backends/memory.ts`

**Step 1: Create MemoryBackend**

Create `packages/sema/src/backends/memory.ts`:

```ts
import type { VFSBackend, VFSHost } from "../vfs.js";

/**
 * Ephemeral VFS backend — no persistence.
 *
 * Files exist only in the WASM memory and are lost on page reload.
 * Use this when you don't need persistence, or for testing.
 *
 * @example
 * ```ts
 * import { SemaInterpreter, MemoryBackend } from "@sema-lang/sema";
 *
 * const sema = await SemaInterpreter.create({
 *   vfs: new MemoryBackend(),
 * });
 * ```
 */
export class MemoryBackend implements VFSBackend {
  async hydrate(_host: VFSHost): Promise<void> {}
  async flush(_host: VFSHost): Promise<void> {}
  async reset(): Promise<void> {}
}
```

**Step 2: Verify build**

Run: `cd packages/sema && npx tsc --noEmit`
Expected: No errors

**Step 3: Commit**

```bash
git add packages/sema/src/backends/memory.ts
git commit -m "feat: add MemoryBackend (ephemeral, no persistence)"
```

---

### Task 3: Add SessionStorageBackend

**Files:**
- Create: `packages/sema/src/backends/session-storage.ts`

**Step 1: Create SessionStorageBackend**

Create `packages/sema/src/backends/session-storage.ts`:

```ts
import type { VFSBackend } from "../vfs.js";
import { WebStorageBackend, type WebStorageBackendOptions } from "./web-storage.js";

export type SessionStorageBackendOptions = WebStorageBackendOptions;

/**
 * VFS backend that persists files to sessionStorage.
 *
 * Files survive page refreshes within the same tab, but are lost when
 * the tab is closed. Good for scratch/draft work.
 *
 * sessionStorage has a ~5–10 MB limit per origin in most browsers.
 *
 * @example
 * ```ts
 * import { SemaInterpreter, SessionStorageBackend } from "@sema-lang/sema";
 *
 * const sema = await SemaInterpreter.create({
 *   vfs: new SessionStorageBackend({ namespace: "my-scratch" }),
 * });
 * ```
 */
export class SessionStorageBackend extends WebStorageBackend implements VFSBackend {
  constructor(opts?: SessionStorageBackendOptions) {
    super(sessionStorage, opts);
  }
}
```

**Step 2: Verify build**

Run: `cd packages/sema && npx tsc --noEmit`
Expected: No errors

**Step 3: Commit**

```bash
git add packages/sema/src/backends/session-storage.ts
git commit -m "feat: add SessionStorageBackend (per-tab persistence)"
```

---

### Task 4: Add IndexedDBBackend

**Files:**
- Create: `packages/sema/src/backends/indexed-db.ts`

**Step 1: Create IndexedDBBackend**

Create `packages/sema/src/backends/indexed-db.ts`:

```ts
import type { VFSBackend, VFSHost } from "../vfs.js";

/** Options for IndexedDBBackend. */
export interface IndexedDBBackendOptions {
  /**
   * Database name. Each namespace gets its own IndexedDB database.
   * @default "sema-vfs"
   */
  namespace?: string;
}

const STORE_NAME = "files";

/**
 * VFS backend that persists files to IndexedDB.
 *
 * Recommended for production use — supports large projects with
 * generous storage limits (typically hundreds of MB per origin).
 * Fully async and doesn't block the main thread.
 *
 * @example
 * ```ts
 * import { SemaInterpreter, IndexedDBBackend } from "@sema-lang/sema";
 *
 * const sema = await SemaInterpreter.create({
 *   vfs: new IndexedDBBackend({ namespace: "my-project" }),
 * });
 *
 * await sema.evalStrAsync(code);
 * await sema.flushVFS(); // persist to IndexedDB
 * ```
 */
export class IndexedDBBackend implements VFSBackend {
  private dbName: string;
  private db: IDBDatabase | null = null;

  constructor(opts?: IndexedDBBackendOptions) {
    this.dbName = opts?.namespace ?? "sema-vfs";
  }

  async init(): Promise<void> {
    this.db = await this.openDB();
  }

  async hydrate(host: VFSHost): Promise<void> {
    if (!this.db) this.db = await this.openDB();

    const records = await this.getAll();

    // Restore directories first (sorted by depth so parents come first)
    const dirs = records
      .filter((r) => r.isDir)
      .map((r) => r.path)
      .sort((a, b) => a.split("/").length - b.split("/").length);
    for (const dir of dirs) {
      host.mkdir(dir);
    }

    // Then restore files
    for (const record of records) {
      if (!record.isDir && record.content !== undefined) {
        host.writeFile(record.path, record.content);
      }
    }
  }

  async flush(host: VFSHost): Promise<void> {
    if (!this.db) this.db = await this.openDB();

    const files = this.collectFiles(host, "/");
    const dirs = this.collectDirs(host, "/");

    const tx = this.db.transaction(STORE_NAME, "readwrite");
    const store = tx.objectStore(STORE_NAME);

    // Clear existing data
    store.clear();

    // Write directories
    for (const dir of dirs) {
      store.put({ path: dir, isDir: true });
    }

    // Write files
    for (const path of files) {
      const content = host.readFile(path);
      if (content !== null) {
        store.put({ path, content, isDir: false });
      }
    }

    await this.txComplete(tx);
  }

  async reset(): Promise<void> {
    if (!this.db) this.db = await this.openDB();

    const tx = this.db.transaction(STORE_NAME, "readwrite");
    tx.objectStore(STORE_NAME).clear();
    await this.txComplete(tx);
  }

  // ── Private helpers ────────────────────────────────────────────

  private openDB(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(this.dbName, 1);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains(STORE_NAME)) {
          db.createObjectStore(STORE_NAME, { keyPath: "path" });
        }
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  private getAll(): Promise<Array<{ path: string; content?: string; isDir: boolean }>> {
    return new Promise((resolve, reject) => {
      const tx = this.db!.transaction(STORE_NAME, "readonly");
      const req = tx.objectStore(STORE_NAME).getAll();
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  private txComplete(tx: IDBTransaction): Promise<void> {
    return new Promise((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
  }

  private collectFiles(host: VFSHost, dir: string): string[] {
    const result: string[] = [];
    const entries = host.listFiles(dir);
    for (const name of entries) {
      const full = dir === "/" ? "/" + name : dir + "/" + name;
      if (host.isDirectory(full)) {
        result.push(...this.collectFiles(host, full));
      } else {
        result.push(full);
      }
    }
    return result;
  }

  private collectDirs(host: VFSHost, dir: string): string[] {
    const result: string[] = [];
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
```

**Step 2: Verify build**

Run: `cd packages/sema && npx tsc --noEmit`
Expected: No errors

**Step 3: Commit**

```bash
git add packages/sema/src/backends/indexed-db.ts
git commit -m "feat: add IndexedDBBackend (production-grade persistence)"
```

---

### Task 5: Update Package Exports

**Files:**
- Modify: `packages/sema/src/index.ts`

**Step 1: Add all backend exports to index.ts**

At the bottom of `packages/sema/src/index.ts`, replace the current export lines (lines 428-430) with:

```ts
export type { VFSBackend, VFSHost } from "./vfs.js";
export { MemoryBackend } from "./backends/memory.js";
export { LocalStorageBackend } from "./backends/local-storage.js";
export type { LocalStorageBackendOptions } from "./backends/local-storage.js";
export { SessionStorageBackend } from "./backends/session-storage.js";
export type { SessionStorageBackendOptions } from "./backends/session-storage.js";
export { IndexedDBBackend } from "./backends/indexed-db.js";
export type { IndexedDBBackendOptions } from "./backends/indexed-db.js";
```

Also update the `VFSBackend` JSDoc comment on the `vfs` option in `InterpreterOptions` (around line 68-78) to mention all backends:

```ts
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
```

**Step 2: Verify build**

Run: `cd packages/sema && npx tsc --noEmit`
Expected: No errors

**Step 3: Commit**

```bash
git add packages/sema/src/index.ts
git commit -m "feat: export all VFS backends from package entrypoint"
```

---

### Task 6: Update VFSBackend Interface JSDoc

**Files:**
- Modify: `packages/sema/src/vfs.ts`

**Step 1: Update the VFSBackend JSDoc to list all built-in backends**

Replace the JSDoc comment on `VFSBackend` (lines 16-33) with:

```ts
/**
 * Pluggable VFS storage backend.
 *
 * Implement this interface to persist VFS state across page reloads.
 * The backend runs outside the eval loop, so async is allowed.
 *
 * Built-in implementations:
 * - {@link MemoryBackend} — ephemeral, no persistence
 * - {@link LocalStorageBackend} — persist to localStorage (~5 MB limit)
 * - {@link SessionStorageBackend} — persist within the tab session
 * - {@link IndexedDBBackend} — persist to IndexedDB (recommended for production)
 *
 * @example
 * ```ts
 * import { SemaInterpreter, IndexedDBBackend } from "@sema-lang/sema";
 *
 * const sema = await SemaInterpreter.create({
 *   vfs: new IndexedDBBackend({ namespace: "my-app" }),
 * });
 * await sema.evalStrAsync(code);
 * await sema.flushVFS(); // persist changes
 * ```
 */
```

**Step 2: Commit**

```bash
git add packages/sema/src/vfs.ts
git commit -m "docs: update VFSBackend JSDoc to list all built-in backends"
```

---

### Task 7: Update npm README

**Files:**
- Modify: `packages/sema/README.md`

**Step 1: Add VFS Persistence section**

After the existing "Virtual Filesystem" section (after line 138, before "## Sandbox"), add:

```markdown
## VFS Persistence

By default, VFS files are lost on page reload. Use a backend to persist them:

```js
import { SemaInterpreter, IndexedDBBackend } from "@sema-lang/sema";

const sema = await SemaInterpreter.create({
  vfs: new IndexedDBBackend({ namespace: "my-project" }),
});

// Files written by Sema code are persisted after flush
await sema.evalStrAsync('(file/write "/hello.txt" "Hello!")');
await sema.flushVFS();

// On next page load, files are automatically restored
```

### Built-in Backends

| Backend | Persistence | Size Limit | Best For |
|---------|-------------|------------|----------|
| `MemoryBackend` | None (lost on reload) | WASM quota only | Testing, ephemeral sandboxes |
| `LocalStorageBackend` | Across page loads | ~5–10 MB per origin | Small projects |
| `SessionStorageBackend` | Within tab session | ~5–10 MB per origin | Scratch work, drafts |
| `IndexedDBBackend` | Across page loads | Hundreds of MB | **Production use** |

All backends accept a `{ namespace }` option (default: `"sema-vfs"`) to isolate storage between different apps or interpreter instances.

### Custom Backends

Implement the `VFSBackend` interface to use any storage mechanism:

```ts
import type { VFSBackend, VFSHost } from "@sema-lang/sema";

class MyBackend implements VFSBackend {
  async init() { /* open connections */ }
  async hydrate(host: VFSHost) { /* restore files into host */ }
  async flush(host: VFSHost) { /* save files from host */ }
  async reset() { /* clear storage */ }
}
```
```

**Step 2: Update the API table** 

Add `vfs` to the `create(opts?)` options table (after line 57):

Replace the create options table with:

```markdown
| Option | Default | Description |
|--------|---------|-------------|
| `wasmUrl` | auto | URL to the `.wasm` binary |
| `stdlib` | `true` | Include the standard library |
| `deny` | `[]` | Capabilities to deny: `"network"`, `"fs-read"`, `"fs-write"` |
| `vfs` | none | VFS backend for persistence: `MemoryBackend`, `LocalStorageBackend`, `SessionStorageBackend`, `IndexedDBBackend` |
```

Add `flushVFS` and `resetVFSAndBackend` to the API section (after `resetVFS()`):

```markdown
### `flushVFS()` → `Promise<void>`

Persist VFS changes to the configured backend. No-op if no backend was provided.

### `resetVFSAndBackend()` → `Promise<void>`

Clear the VFS and the persistent backend storage.
```

**Step 3: Commit**

```bash
git add packages/sema/README.md
git commit -m "docs: add VFS persistence section to npm README"
```

---

### Task 8: Update Website Docs (embedding-js.md)

**Files:**
- Modify: `website/docs/embedding-js.md`

**Step 1: Add VFS Persistence section**

After the "Quota Management" subsection (line 369), before "## Real-World Example", add a new `## VFS Persistence` section:

```markdown
## VFS Persistence

By default, VFS files live only in WASM memory and are lost on page reload. To persist files across sessions, pass a **VFS backend** when creating the interpreter:

```js
import { SemaInterpreter, IndexedDBBackend } from "@sema-lang/sema";

const sema = await SemaInterpreter.create({
  vfs: new IndexedDBBackend({ namespace: "my-project" }),
});

// Sema code can read/write files as usual
await sema.evalStrAsync('(file/write "/config.json" "{\\"theme\\": \\"dark\\"}")');

// Persist current VFS state to the backend
await sema.flushVFS();

// On next page load, files are automatically restored via hydrate()
```

### Built-in Backends

Four backends ship with the `@sema-lang/sema` package:

| Backend | Import | Persistence | Limit |
| --- | --- | --- | --- |
| `MemoryBackend` | `@sema-lang/sema` | None — lost on reload | WASM quota only |
| `LocalStorageBackend` | `@sema-lang/sema` | Across page loads, per origin | ~5–10 MB |
| `SessionStorageBackend` | `@sema-lang/sema` | Within the current tab | ~5–10 MB |
| `IndexedDBBackend` | `@sema-lang/sema` | Across page loads, per origin | Hundreds of MB |

::: tip Choosing a Backend
Use **`IndexedDBBackend`** for production apps — it handles large file sets and doesn't compete with other localStorage usage. Use **`LocalStorageBackend`** for quick prototypes. Use **`MemoryBackend`** (or no backend) when persistence isn't needed.
:::

### Backend Options

All backends accept a `namespace` option to isolate storage:

```js
// Two interpreters with separate persistent storage
const editor = await SemaInterpreter.create({
  vfs: new IndexedDBBackend({ namespace: "editor-files" }),
});
const preview = await SemaInterpreter.create({
  vfs: new IndexedDBBackend({ namespace: "preview-files" }),
});
```

### Flush and Reset

```js
// Persist VFS to the backend (call after eval)
await sema.flushVFS();

// Clear both VFS memory and persistent storage
await sema.resetVFSAndBackend();
```

### Custom Backends

Implement the `VFSBackend` interface to persist files anywhere — a remote API, a service worker cache, or a custom database:

```ts
import type { VFSBackend, VFSHost } from "@sema-lang/sema";

class CloudBackend implements VFSBackend {
  async init() {
    // Open connections, authenticate, etc.
  }

  async hydrate(host: VFSHost) {
    // Fetch files from your API and write them into the WASM VFS
    const files = await fetch("/api/files").then(r => r.json());
    for (const { path, content } of files) {
      host.writeFile(path, content);
    }
  }

  async flush(host: VFSHost) {
    // Read files from the WASM VFS and upload them
    const paths = host.listFiles("/");
    for (const name of paths) {
      const content = host.readFile("/" + name);
      if (content !== null) {
        await fetch("/api/files/" + name, {
          method: "PUT",
          body: content,
        });
      }
    }
  }

  async reset() {
    await fetch("/api/files", { method: "DELETE" });
  }
}

const sema = await SemaInterpreter.create({
  vfs: new CloudBackend(),
});
```

::: info VFSHost API
The `host` object passed to `hydrate()` and `flush()` provides these methods:
- `readFile(path)` → `string | null`
- `writeFile(path, content)` — write or overwrite a file
- `deleteFile(path)` → `boolean`
- `mkdir(path)` — create directories recursively
- `listFiles(dir)` → `string[]` — list entries in a directory
- `fileExists(path)` → `boolean`
- `isDirectory(path)` → `boolean`
- `resetVFS()` — clear all files and directories
:::
```

**Step 2: Update the API Reference table at the bottom**

Add these rows to the API reference table (after the `interp.resetVFS()` row):

```markdown
| `interp.flushVFS()` | Persist VFS to the configured backend. Returns `Promise<void>`. |
| `interp.resetVFSAndBackend()` | Clear VFS and persistent backend. Returns `Promise<void>`. |
| `MemoryBackend` | Ephemeral VFS backend — no persistence |
| `LocalStorageBackend` | Persist VFS to `localStorage` (~5 MB limit) |
| `SessionStorageBackend` | Persist VFS to `sessionStorage` (per-tab) |
| `IndexedDBBackend` | Persist VFS to IndexedDB (recommended for production) |
| `VFSBackend` | Interface for custom backends: `{ init?, hydrate, flush, reset? }` |
| `VFSHost` | Host bridge passed to backends: `{ readFile, writeFile, deleteFile, mkdir, listFiles, fileExists, isDirectory, resetVFS }` |
```

**Step 3: Update the Limitations table**

In the Limitations table, update the "Filesystem" row from:

```
| Filesystem | Real filesystem | In-memory VFS (1 MB/file, 16 MB total) |
```

to:

```
| Filesystem | Real filesystem | In-memory VFS with pluggable persistence (1 MB/file, 16 MB total) |
```

And update the "No filesystem" workaround to:

```
- **Persistence**: Use a VFS backend (`IndexedDBBackend`, `LocalStorageBackend`) to persist files across page reloads. See [VFS Persistence](#vfs-persistence).
```

**Step 4: Commit**

```bash
git add website/docs/embedding-js.md
git commit -m "docs: add VFS Persistence section to JS embedding docs"
```

---

### Task 9: Final Build Verification

**Step 1: Full TypeScript build**

Run: `cd packages/sema && npx tsc --noEmit`
Expected: No errors

**Step 2: Verify all exports resolve**

Run: `cd packages/sema && npx tsc`
Expected: Generates `dist/` with all `.js` and `.d.ts` files including all backend files

**Step 3: Check the dist output has all backends**

Run: `ls packages/sema/dist/backends/`
Expected: `memory.js`, `memory.d.ts`, `web-storage.js`, `web-storage.d.ts`, `local-storage.js`, `local-storage.d.ts`, `session-storage.js`, `session-storage.d.ts`, `indexed-db.js`, `indexed-db.d.ts`

**Step 4: Commit build output if needed**

(dist/ is likely gitignored — no commit needed)
