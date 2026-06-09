# VFS Backend Swapping Demo — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a VFS backend selector to the VFS demo page so users can swap between In-Memory, LocalStorage, and SessionStorage backends, showcasing the pluggable VFSBackend architecture.

**Architecture:** Create a `vfs-backends.js` ES module with three backend classes (matching the VFSBackend contract from the TS wrapper), a VFSHost bridge factory wrapping the raw WASM interpreter, and a backend orchestrator. Add a `<select>` dropdown in the File Explorer pane header. Auto-flush after every Run for persistent backends.

**Tech Stack:** Plain JS ES modules (no build step), raw WASM `SemaInterpreter` bindings, localStorage/sessionStorage APIs.

---

### Task 1: Create `vfs-backends.js` — VFSHost bridge + 3 backend classes

**Files:**
- Create: `playground/vfs-demo/vfs-backends.js`

**Step 1: Write the VFSHost bridge factory and InMemoryBackend**

```js
// playground/vfs-demo/vfs-backends.js

/**
 * Creates a VFSHost bridge that wraps the raw WASM SemaInterpreter methods.
 * This mirrors packages/sema/src/index.ts _vfsHost().
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

/** In-memory backend — ephemeral, no persistence. */
export class InMemoryBackend {
  get label() { return 'In-Memory'; }
  get description() { return 'Ephemeral — cleared on page refresh'; }
  async hydrate(_host) { /* nothing to restore */ }
  async flush(_host) { /* nothing to persist */ }
  async reset() { /* nothing to clear */ }
}
```

**Step 2: Add WebStorageBackend (covers both localStorage and sessionStorage)**

Append to `vfs-backends.js`:

```js
/** Backend that persists to a Web Storage API (localStorage or sessionStorage). */
class WebStorageBackend {
  constructor(storage, opts = {}) {
    this._storage = storage;
    const ns = opts.namespace ?? 'sema-vfs';
    this._filePrefix = ns + ':f:';
    this._dirsKey = ns + ':__dirs__';
  }

  async hydrate(host) {
    // Restore directories first
    const dirsJson = this._storage.getItem(this._dirsKey);
    if (dirsJson) {
      try {
        for (const dir of JSON.parse(dirsJson)) host.mkdir(dir);
      } catch { /* ignore corrupt data */ }
    }
    // Restore files
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
    // Clear old entries
    const toRemove = [];
    for (let i = 0; i < this._storage.length; i++) {
      const key = this._storage.key(i);
      if (key && (key.startsWith(this._filePrefix) || key === this._dirsKey)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) this._storage.removeItem(key);

    // Write current files + dirs manifest
    const files = this._collectFiles(host, '/');
    for (const path of files) {
      const content = host.readFile(path);
      if (content !== null) this._storage.setItem(this._filePrefix + path, content);
    }
    this._storage.setItem(this._dirsKey, JSON.stringify(this._collectDirs(host, '/')));
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

  _collectFiles(host, dir) {
    const result = [];
    for (const name of host.listFiles(dir)) {
      const full = dir === '/' ? '/' + name : dir + '/' + name;
      if (host.isDirectory(full)) result.push(...this._collectFiles(host, full));
      else result.push(full);
    }
    return result;
  }

  _collectDirs(host, dir) {
    const result = [];
    for (const name of host.listFiles(dir)) {
      const full = dir === '/' ? '/' + name : dir + '/' + name;
      if (host.isDirectory(full)) {
        result.push(full);
        result.push(...this._collectDirs(host, full));
      }
    }
    return result;
  }
}
```

**Step 3: Add LocalStorageBackend, SessionStorageBackend, and factory**

```js
/** Persists VFS to localStorage — survives page refreshes. */
export class LocalStorageBackend extends WebStorageBackend {
  get label() { return 'LocalStorage'; }
  get description() { return 'Persists across page refreshes'; }
  constructor(opts) { super(localStorage, { namespace: 'sema-vfs-demo', ...opts }); }
}

/** Persists VFS to sessionStorage — survives within tab only. */
export class SessionStorageBackend extends WebStorageBackend {
  get label() { return 'SessionStorage'; }
  get description() { return 'Persists within this tab session'; }
  constructor(opts) { super(sessionStorage, { namespace: 'sema-vfs-demo-session', ...opts }); }
}

/** Registry of available backends. */
export const BACKENDS = {
  memory: () => new InMemoryBackend(),
  local: () => new LocalStorageBackend(),
  session: () => new SessionStorageBackend(),
};
```

**Step 4: Verify it loads without errors**

Open browser console, run:
```js
import('./vfs-backends.js').then(m => console.log(Object.keys(m.BACKENDS)));
// Expected: ["memory", "local", "session"]
```

**Step 5: Commit**

```bash
git add playground/vfs-demo/vfs-backends.js
git commit -m "feat(vfs-demo): add VFS backend classes (in-memory, localStorage, sessionStorage)"
```

---

### Task 2: Add backend selector UI to index.html + style.css

**Files:**
- Modify: `playground/vfs-demo/index.html` (File Explorer pane header)
- Modify: `playground/vfs-demo/style.css` (backend selector styles)

**Step 1: Add select dropdown to File Explorer header**

In `index.html`, replace the File Explorer pane header (lines ~33-36) with:

```html
<div class="file-tree-pane">
  <div class="pane-header">
    <span class="pane-title">File Explorer</span>
    <select id="backend-select" class="backend-select" data-testid="backend-select">
      <option value="memory">In-Memory</option>
      <option value="local">LocalStorage</option>
      <option value="session">SessionStorage</option>
    </select>
  </div>
  <div class="file-tree" id="file-tree" data-testid="file-tree"></div>
</div>
```

**Step 2: Add backend select styles to style.css**

Add after the `.tree-empty` styles:

```css
/* ── Backend selector ── */
.backend-select {
  font-family: var(--mono);
  font-size: 0.65rem;
  color: var(--text);
  background: var(--bg);
  border: 1px solid var(--border);
  padding: 0.2rem 0.4rem;
  border-radius: 3px;
  cursor: pointer;
  outline: none;
  transition: border-color 0.15s;
  appearance: none;
  -webkit-appearance: none;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='8' height='5' fill='%235a5448'%3E%3Cpath d='M0 0l4 5 4-5z'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 0.4rem center;
  padding-right: 1.2rem;
}
.backend-select:hover { border-color: var(--border-focus); }
.backend-select:focus { border-color: var(--gold-dim); }
.backend-select option {
  background: var(--bg);
  color: var(--text);
}
```

**Step 3: Verify layout looks correct**

Open http://localhost:8787/vfs-demo/ and check select is visible in file explorer header, styled correctly with dark background and gold focus.

**Step 4: Commit**

```bash
git add playground/vfs-demo/index.html playground/vfs-demo/style.css
git commit -m "feat(vfs-demo): add backend selector dropdown UI"
```

---

### Task 3: Wire up backend swapping logic in app.js

**Files:**
- Modify: `playground/vfs-demo/app.js`

**Step 1: Import backends and add state variables**

Replace line 1 and add state after line 4:

```js
import init, { SemaInterpreter } from '../pkg/sema_wasm.js';
import { makeVfsHost, BACKENDS } from './vfs-backends.js';

let interp = null;
let activeFilePath = null;
let vfsHost = null;
let vfsBackend = null;
let backendName = 'memory';
```

Add element ref after other element refs:

```js
const backendSelect = document.getElementById('backend-select');
```

**Step 2: Update `start()` to initialize with stored backend preference**

Replace the `start()` function:

```js
const BACKEND_PREF_KEY = 'sema-vfs-demo:backend';

async function start() {
  await init();
  interp = new SemaInterpreter();
  vfsHost = makeVfsHost(interp);
  versionEl.textContent = `v${interp.version()}`;

  // Restore backend preference
  const storedBackend = localStorage.getItem(BACKEND_PREF_KEY) ?? 'memory';
  if (BACKENDS[storedBackend]) {
    backendName = storedBackend;
    backendSelect.value = storedBackend;
  }

  // Initialize backend
  vfsBackend = BACKENDS[backendName]();
  await vfsBackend.hydrate(vfsHost);

  runBtn.disabled = false;
  clearVfsBtn.disabled = false;
  loadingEl.classList.add('hidden');
  statusEl.textContent = 'Ready';
  statusEl.className = 'status-text status-ready';
  outputEl.innerHTML = '<div class="output-welcome">Ready. Press Run to evaluate the script.</div>';
  refreshFileTree();
  refreshStats();
}
```

**Step 3: Add auto-flush after Run for persistent backends**

In the `run()` function, after `refreshStats()` and before the re-read of active file, add:

```js
  // Auto-flush for persistent backends
  if (backendName !== 'memory') {
    try {
      await vfsBackend.flush(vfsHost);
    } catch (e) {
      statusEl.textContent = `Persist failed: ${e.message}`;
      statusEl.className = 'status-text status-loading';
    }
  }
```

**Step 4: Add backend swap handler**

Add after the `clearVfsBtn` event listener:

```js
backendSelect.addEventListener('change', async () => {
  const newName = backendSelect.value;
  if (newName === backendName) return;

  statusEl.textContent = 'Switching backend…';
  statusEl.className = 'status-text status-loading';

  // Create new backend
  const newBackend = BACKENDS[newName]();

  // Reset VFS and hydrate from new backend
  interp.resetVFS();
  await newBackend.hydrate(vfsHost);

  // Update state
  vfsBackend = newBackend;
  backendName = newName;
  localStorage.setItem(BACKEND_PREF_KEY, newName);

  // Reset viewer
  activeFilePath = null;
  viewerTitle.textContent = 'File Viewer';
  fileViewerEl.innerHTML = '<span class="viewer-placeholder">Click a file in the explorer to view its contents.</span>';

  refreshFileTree();
  refreshStats();
  statusEl.textContent = 'Ready';
  statusEl.className = 'status-text status-ready';
});
```

**Step 5: Update Clear VFS to also clear backend**

Replace the `clearVfsBtn` event listener:

```js
clearVfsBtn.addEventListener('click', async () => {
  interp.resetVFS();
  await vfsBackend.reset?.();
  activeFilePath = null;
  viewerTitle.textContent = 'File Viewer';
  fileViewerEl.innerHTML = '<span class="viewer-placeholder">Click a file in the explorer to view its contents.</span>';
  refreshFileTree();
  refreshStats();
});
```

**Step 6: Verify full flow**

1. Open demo, select "LocalStorage", Run script, refresh page — files should persist
2. Switch to "In-Memory" — file tree clears (LocalStorage backend had files, in-memory doesn't)
3. Switch back to "LocalStorage" — files restored from localStorage
4. Select "SessionStorage", Run, open new tab to same URL — no files (session-scoped)
5. Click "Clear VFS" while on LocalStorage — files gone and localStorage cleared

**Step 7: Commit**

```bash
git add playground/vfs-demo/app.js
git commit -m "feat(vfs-demo): wire up backend swapping with auto-flush and persistence"
```

---

### Task 4: Show active backend info in the status bar

**Files:**
- Modify: `playground/vfs-demo/index.html` (status bar)
- Modify: `playground/vfs-demo/app.js` (update status display)

**Step 1: Add backend info span to status bar in index.html**

In the status bar div, add a span for backend info between status-text and vfs-stats:

```html
<div class="status-bar" data-testid="status">
  <span class="status-text status-loading" id="status-text">Loading WASM module…</span>
  <span class="status-text" id="backend-info" data-testid="backend-info"></span>
  <span class="status-text" id="vfs-stats" data-testid="vfs-stats"></span>
</div>
```

**Step 2: Add backendInfoEl ref and updateBackendInfo() in app.js**

```js
const backendInfoEl = document.getElementById('backend-info');

function updateBackendInfo() {
  const labels = { memory: '⚡ In-Memory', local: '💾 LocalStorage', session: '📋 SessionStorage' };
  backendInfoEl.textContent = labels[backendName] ?? backendName;
}
```

**Step 3: Call updateBackendInfo() in start() and backend change handler**

Add `updateBackendInfo()` call at end of `start()` and after `backendName = newName` in the change handler.

**Step 4: Commit**

```bash
git add playground/vfs-demo/index.html playground/vfs-demo/app.js
git commit -m "feat(vfs-demo): show active backend indicator in status bar"
```
