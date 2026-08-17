/**
 * VFS backend classes for the VFS demo.
 *
 * Mirrors the VFSBackend/VFSHost contract from packages/sema/src/vfs.ts,
 * implemented as plain JS ES modules (no build step required).
 */

/**
 * Creates a VFSHost bridge wrapping the raw WASM SemaInterpreter methods.
 * @param {import('../pkg/sema_wasm.js').SemaInterpreter} interp
 */
export function makeVfsHost(interp) {
  return {
    readFile: (p) => {
      const r = interp.readFile(p);
      return r === null || r === undefined ? null : r;
    },
    writeFile: (p, c) => {
      const err = interp.writeFile(p, c);
      if (typeof err === 'string') throw new Error(err);
    },
    deleteFile: (p) => !!interp.deleteFile(p),
    mkdir: (p) => interp.mkdir(p),
    listFiles: (d) => Array.from(interp.listFiles(d ?? '/')),
    fileExists: (p) => interp.fileExists(p),
    isDirectory: (p) => interp.isDirectory(p),
    resetVFS: () => interp.resetVFS(),
  };
}

// ── Shared collectors ─────────────────────────────────────────────────

/**
 * Recursively collect the absolute paths of every file under `dir`.
 * @param {{listFiles: Function, isDirectory: Function}} host
 * @param {string} dir
 */
function collectFiles(host, dir) {
  const result = [];
  for (const name of host.listFiles(dir)) {
    const full = dir === '/' ? '/' + name : dir + '/' + name;
    if (host.isDirectory(full)) result.push(...collectFiles(host, full));
    else result.push(full);
  }
  return result;
}

/**
 * Recursively collect the absolute paths of every directory under `dir`
 * (including `dir` itself for non-root calls).
 * @param {{listFiles: Function, isDirectory: Function}} host
 * @param {string} dir
 */
function collectDirs(host, dir) {
  const result = [];
  for (const name of host.listFiles(dir)) {
    const full = dir === '/' ? '/' + name : dir + '/' + name;
    if (host.isDirectory(full)) {
      result.push(full);
      result.push(...collectDirs(host, full));
    }
  }
  return result;
}

// ── Backends ──────────────────────────────────────────────────────────

/** In-memory backend — ephemeral, no persistence. */
export class InMemoryBackend {
  get label() { return 'In-Memory'; }
  get description() { return 'Ephemeral — cleared on page refresh'; }
  async hydrate(_host) {}
  async flush(_host) {}
  async reset() {}
}

/** Backend that persists to a Web Storage API (localStorage or sessionStorage). */
class WebStorageBackend {
  constructor(storage, opts = {}) {
    this._storage = storage;
    const ns = opts.namespace ?? 'sema-vfs';
    this._filePrefix = ns + ':f:';
    this._dirsKey = ns + ':__dirs__';
  }

  async hydrate(host) {
    const dirsJson = this._storage.getItem(this._dirsKey);
    if (dirsJson) {
      try {
        for (const dir of JSON.parse(dirsJson)) host.mkdir(dir);
      } catch { /* ignore corrupt data */ }
    }
    for (let i = 0; i < this._storage.length; i++) {
      const key = this._storage.key(i);
      if (key && key.startsWith(this._filePrefix)) {
        const path = key.slice(this._filePrefix.length);
        const content = this._storage.getItem(key);
        if (content !== null) host.writeFile(path, content);
      }
    }
  }

  async flush(host) {
    const toRemove = [];
    for (let i = 0; i < this._storage.length; i++) {
      const key = this._storage.key(i);
      if (key && (key.startsWith(this._filePrefix) || key === this._dirsKey)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) this._storage.removeItem(key);

    const files = collectFiles(host, '/');
    for (const path of files) {
      const content = host.readFile(path);
      if (content !== null) this._storage.setItem(this._filePrefix + path, content);
    }
    this._storage.setItem(this._dirsKey, JSON.stringify(collectDirs(host, '/')));
  }

  async reset() {
    const toRemove = [];
    for (let i = 0; i < this._storage.length; i++) {
      const key = this._storage.key(i);
      if (key && (key.startsWith(this._filePrefix) || key === this._dirsKey)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) this._storage.removeItem(key);
  }
}

/** Persists VFS to localStorage — survives page refreshes. */
export class LocalStorageBackend extends WebStorageBackend {
  get label() { return 'LocalStorage'; }
  get description() { return 'Persists across page refreshes'; }
  constructor(opts) { super(localStorage, { namespace: 'sema-playground', ...opts }); }
}

/** Persists VFS to sessionStorage — survives within tab only. */
export class SessionStorageBackend extends WebStorageBackend {
  get label() { return 'SessionStorage'; }
  get description() { return 'Persists within this tab session'; }
  constructor(opts) { super(sessionStorage, { namespace: 'sema-playground-session', ...opts }); }
}

/** Persists VFS to IndexedDB — recommended for production use. */
export class IndexedDBBackend {
  get label() { return 'IndexedDB'; }
  get description() { return 'Persists across page loads (recommended for production)'; }

  constructor(opts = {}) {
    this._dbName = opts.namespace ?? 'sema-playground-idb';
    this._db = null;
  }

  async init() {
    this._db = await this._openDB();
  }

  async hydrate(host) {
    const db = this._db ?? await this._openDB();
    const records = await this._getAll(db);

    // Restore directories first (shallowest to deepest)
    const dirs = records
      .filter(r => r.isDir)
      .sort((a, b) => a.path.split('/').length - b.path.split('/').length);
    for (const rec of dirs) host.mkdir(rec.path);

    // Then files
    for (const rec of records) {
      if (!rec.isDir && rec.content !== undefined) {
        host.writeFile(rec.path, rec.content);
      }
    }
  }

  async flush(host) {
    const db = this._db ?? await this._openDB();
    const tx = db.transaction('files', 'readwrite');
    const store = tx.objectStore('files');

    store.clear();

    // Write directories
    for (const dir of collectDirs(host, '/')) {
      store.put({ path: dir, isDir: true });
    }

    // Write files
    for (const filePath of collectFiles(host, '/')) {
      const content = host.readFile(filePath);
      if (content !== null) {
        store.put({ path: filePath, content, isDir: false });
      }
    }

    await this._txComplete(tx);
  }

  async reset() {
    const db = this._db ?? await this._openDB();
    const tx = db.transaction('files', 'readwrite');
    tx.objectStore('files').clear();
    await this._txComplete(tx);
  }

  _openDB() {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(this._dbName, 1);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains('files')) {
          db.createObjectStore('files', { keyPath: 'path' });
        }
      };
      req.onsuccess = () => { this._db = req.result; resolve(req.result); };
      req.onerror = () => reject(req.error);
    });
  }

  _getAll(db) {
    return new Promise((resolve, reject) => {
      const tx = db.transaction('files', 'readonly');
      const req = tx.objectStore('files').getAll();
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  _txComplete(tx) {
    return new Promise((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
  }
}

/** Registry of available backends. */
export const BACKENDS = {
  memory: () => new InMemoryBackend(),
  local: () => new LocalStorageBackend(),
  session: () => new SessionStorageBackend(),
  indexeddb: () => new IndexedDBBackend(),
};
