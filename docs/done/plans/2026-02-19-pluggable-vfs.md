# Pluggable VFS Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Normalize VFS paths to always-leading-slash canonical form, then add a pluggable VFS backend interface (hydrate/flush pattern) with a localStorage reference implementation.

**Architecture:** Two-phase approach: (1) Add a `normalize_path()` helper in Rust and apply it at every VFS entry point so both stores use canonical `/`-prefixed paths. (2) Define a `VFSBackend` TypeScript interface in the `@sema-lang/sema` package with `hydrate()`/`flush()` lifecycle methods, wire it into `SemaInterpreter.create()`, and ship a `LocalStorageBackend` as the first concrete backend.

**Tech Stack:** Rust (sema-wasm crate), TypeScript (packages/sema), Playwright (playground tests)

---

### Task 1: Add `normalize_path()` helper in Rust

**Files:**
- Modify: `crates/sema-wasm/src/lib.rs` (top of file, near `vfs_check_quota`)

**Step 1: Add the normalize function**

Add this function after `vfs_check_quota` (around line 68):

```rust
/// Normalize a VFS path to canonical form: always starts with "/",
/// no trailing slash (except root), collapsed "//", resolved "." segments,
/// ".." rejected (no parent traversal in sandbox).
fn normalize_path(path: &str) -> Result<String, SemaError> {
    let path = path.trim();
    if path.is_empty() || path == "/" {
        return Ok("/".to_string());
    }

    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                return Err(SemaError::eval(
                    "VFS path error: '..' parent traversal not allowed",
                ));
            }
            s => segments.push(s),
        }
    }

    if segments.is_empty() {
        return Ok("/".to_string());
    }

    let mut result = String::with_capacity(path.len() + 1);
    for seg in &segments {
        result.push('/');
        result.push_str(seg);
    }
    Ok(result)
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p sema-wasm`
Expected: compiles with no errors (may warn about unused function)

**Step 3: Commit**

```bash
git add crates/sema-wasm/src/lib.rs
git commit -m "feat(wasm-vfs): add normalize_path() helper for canonical VFS paths"
```

---

### Task 2: Apply normalize_path to all Sema-side VFS functions

**Files:**
- Modify: `crates/sema-wasm/src/lib.rs` (the `register_wasm_builtins` function)

Apply `normalize_path()` to every Sema native function that takes a file path. Each function extracts `path` (or `from`/`to`/`src`/`dest`/`dir`) as `&str` — add a `let path = &normalize_path(path)?;` line right after extracting the string.

**Functions to modify** (all inside `register_wasm_builtins`):

1. **`file/read`** (~line 1097): After `let path = args[0].as_str()...`, add:
   ```rust
   let path = &normalize_path(path)?;
   ```

2. **`file/write`** (~line 1115): After `let path = args[0].as_str()...`, add:
   ```rust
   let path = &normalize_path(path)?;
   ```

3. **`file/exists?`** (~line 1144): After `let path = args[0].as_str()...`, add:
   ```rust
   let path = &normalize_path(path)?;
   ```

4. **`file/delete`** (~line 1159): After `let path = args[0].as_str()...`, add:
   ```rust
   let path = &normalize_path(path)?;
   ```

5. **`file/rename`** (~line 1178): After extracting `from` and `to`, add:
   ```rust
   let from = &normalize_path(from)?;
   let to = &normalize_path(to)?;
   ```

6. **`file/list`** (~line 1207): After `let dir = args[0].as_str()...`, add:
   ```rust
   let dir = &normalize_path(dir)?;
   ```

7. **`file/mkdir`** (~line 1245): After `let path = args[0].as_str()...`, add:
   ```rust
   let path = &normalize_path(path)?;
   ```
   Also simplify the mkdir body since normalize_path already handles the format:
   ```rust
   VFS_DIRS.with(|dirs| {
       let mut set = dirs.borrow_mut();
       // Insert each ancestor: /a, /a/b, /a/b/c
       let mut current = String::new();
       for seg in path.strip_prefix('/').unwrap_or(path).split('/') {
           current.push('/');
           current.push_str(seg);
           set.insert(current.clone());
       }
   });
   ```

8. **`file/is-directory?`** (~line 1275): After `let path = args[0].as_str()...`, add:
   ```rust
   let path = &normalize_path(path)?;
   ```

9. **`file/is-file?`** (~line 1290): After `let path = args[0].as_str()...`, add:
   ```rust
   let path = &normalize_path(path)?;
   ```

10. **`file/append`** (~line 1316): After `let path = args[0].as_str()...`, add:
    ```rust
    let path = &normalize_path(path)?;
    ```

11. **`file/copy`** (~line 1342): After extracting `src` and `dest`, add:
    ```rust
    let src = &normalize_path(src)?;
    let dest = &normalize_path(dest)?;
    ```

12. **`file/read-lines`** (~line 1381): After `let path = args[0].as_str()...`, add:
    ```rust
    let path = &normalize_path(path)?;
    ```

13. **`file/write-lines`** (~line 1399): After `let path = args[0].as_str()...`, add:
    ```rust
    let path = &normalize_path(path)?;
    ```

**Step 2: Verify it compiles**

Run: `cargo check -p sema-wasm`
Expected: compiles cleanly

**Step 3: Commit**

```bash
git add crates/sema-wasm/src/lib.rs
git commit -m "feat(wasm-vfs): normalize paths in all Sema-side file/* functions"
```

---

### Task 3: Apply normalize_path to all JS-side VFS methods

**Files:**
- Modify: `crates/sema-wasm/src/lib.rs` (the `impl WasmInterpreter` / `SemaInterpreter` block)

Apply `normalize_path()` to all `#[wasm_bindgen]` VFS methods:

1. **`read_file`** (~line 2078):
   ```rust
   pub fn read_file(&self, path: &str) -> JsValue {
       let path = match normalize_path(path) {
           Ok(p) => p,
           Err(_) => return JsValue::NULL,
       };
       VFS.with(|vfs| match vfs.borrow().get(&path) {
   ```

2. **`write_file`** (~line 2087):
   ```rust
   pub fn write_file(&self, path: &str, content: &str) -> JsValue {
       let path = match normalize_path(path) {
           Ok(p) => p,
           Err(e) => return JsValue::from_str(&format!("{}", e.inner())),
       };
       match vfs_check_quota(&path, content.len()) {
   ```
   Also update the `map.insert(path.to_string(), ...)` to use `path.clone()` or just `path` since it's now a `String`.

3. **`delete_file`** (~line 2108):
   ```rust
   pub fn delete_file(&self, path: &str) -> bool {
       let path = match normalize_path(path) {
           Ok(p) => p,
           Err(_) => return false,
       };
       VFS.with(|vfs| match vfs.borrow_mut().remove(&path) {
   ```

4. **`list_files`** (~line 2121): Normalize the `dir` parameter:
   ```rust
   pub fn list_files(&self, dir: &str) -> JsValue {
       let dir = match normalize_path(dir) {
           Ok(p) => p,
           Err(_) => return js_sys::Array::new().into(),
       };
       let prefix = if dir == "/" {
           "/".to_string()
       } else {
           format!("{dir}/")
       };
   ```

5. **`file_exists`** (~line 2159):
   ```rust
   pub fn file_exists(&self, path: &str) -> bool {
       let path = match normalize_path(path) {
           Ok(p) => p,
           Err(_) => return false,
       };
       let in_vfs = VFS.with(|vfs| vfs.borrow().contains_key(&path));
       let in_dirs = VFS_DIRS.with(|dirs| dirs.borrow().contains(&path));
   ```

6. **`mkdir`** (~line 2166): Normalize and simplify:
   ```rust
   pub fn mkdir(&self, path: &str) {
       let path = match normalize_path(path) {
           Ok(p) => p,
           Err(_) => return,
       };
       VFS_DIRS.with(|dirs| {
           let mut set = dirs.borrow_mut();
           let mut current = String::new();
           for seg in path.strip_prefix('/').unwrap_or(&path).split('/') {
               current.push('/');
               current.push_str(seg);
               set.insert(current.clone());
           }
       });
   }
   ```

7. **`is_directory`** (~line 2188):
   ```rust
   pub fn is_directory(&self, path: &str) -> bool {
       let path = match normalize_path(path) {
           Ok(p) => p,
           Err(_) => return false,
       };
       VFS_DIRS.with(|dirs| dirs.borrow().contains(&path))
   }
   ```

**Step 2: Verify it compiles**

Run: `cargo check -p sema-wasm`

**Step 3: Commit**

```bash
git add crates/sema-wasm/src/lib.rs
git commit -m "feat(wasm-vfs): normalize paths in all JS-side VFS methods"
```

---

### Task 4: Simplify the VFS demo app.js (path normalization is now in Rust)

**Files:**
- Modify: `playground/vfs-demo/app.js`

Now that Rust normalizes all paths, the JS side can use simple paths everywhere. Replace the complex dual-query `buildTree` with a clean version:

```javascript
function buildTree(dir) {
  let entries;
  try { entries = interp.listFiles(dir); } catch { return []; }
  if (!entries || entries.length === 0) return [];

  const items = [];
  for (const name of entries) {
    const fullPath = dir === '/' ? '/' + name : dir + '/' + name;
    const isDir = interp.isDirectory(fullPath);
    items.push({ name, fullPath, isDir, children: isDir ? buildTree(fullPath) : null });
  }

  items.sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    return a.name.localeCompare(b.name);
  });

  return items;
}
```

Update `renderTree` to use `item.fullPath` consistently for both `viewFile` and the active class check. Update `viewFile` to take a single `path` argument.

**Step 2: Rebuild WASM and test**

```bash
cd crates/sema-wasm && wasm-pack build --target web --out-dir ../../playground/pkg
cd playground && npx playwright test tests/vfs-demo.spec.ts
```

**Step 3: Commit**

```bash
git add playground/vfs-demo/app.js
git commit -m "refactor(vfs-demo): simplify file tree now that Rust normalizes paths"
```

---

### Task 5: Run integration tests to verify path normalization doesn't break anything

**Files:**
- Read: `crates/sema/tests/integration_test.rs` (check for existing VFS/file tests)

**Step 1: Build WASM**

```bash
wasm-pack build crates/sema-wasm --target web --out-dir ../../playground/pkg
```

**Step 2: Run all Playwright tests**

```bash
cd playground && npx playwright test
```

**Step 3: Run Rust tests (non-WASM crates should be unaffected)**

```bash
cargo test
```

Expected: all tests pass. If any fail, fix before proceeding.

**Step 4: Commit any fixes**

---

### Task 6: Define VFSBackend and VFSHost TypeScript interfaces

**Files:**
- Create: `packages/sema/src/vfs.ts`

```typescript
/**
 * Host bridge — methods the backend calls to read/write the in-memory WASM VFS.
 * Provided by SemaInterpreter, not implemented by users.
 */
export interface VFSHost {
  readFile(path: string): string | null;
  writeFile(path: string, content: string): void;
  deleteFile(path: string): boolean;
  mkdir(path: string): void;
  listFiles(dir: string): string[];
  fileExists(path: string): boolean;
  isDirectory(path: string): boolean;
  resetVFS(): void;
}

/**
 * Pluggable VFS storage backend.
 *
 * Implement this interface to persist VFS state across page reloads.
 * The backend runs outside the eval loop, so async is allowed.
 *
 * Built-in implementations:
 * - {@link LocalStorageBackend} — persist to localStorage
 *
 * @example
 * ```ts
 * const sema = await SemaInterpreter.create({
 *   vfs: new LocalStorageBackend({ namespace: "my-app" }),
 * });
 * await sema.evalStrAsync(code);
 * await sema.flushVFS(); // persist changes
 * ```
 */
export interface VFSBackend {
  /** Optional: open DB connections, request permissions, etc. */
  init?(): Promise<void>;

  /**
   * Populate the in-memory WASM VFS from persistent storage.
   * Called once during `SemaInterpreter.create()`.
   */
  hydrate(host: VFSHost): Promise<void>;

  /**
   * Persist current in-memory VFS state to storage.
   * Called explicitly via `sema.flushVFS()`.
   */
  flush(host: VFSHost): Promise<void>;

  /** Optional: clear all persistent data. */
  reset?(): Promise<void>;
}
```

**Step 2: Verify TypeScript compiles**

```bash
cd packages/sema && npx tsc --noEmit
```

**Step 3: Commit**

```bash
git add packages/sema/src/vfs.ts
git commit -m "feat(sema): define VFSBackend and VFSHost interfaces"
```

---

### Task 7: Implement LocalStorageBackend

**Files:**
- Create: `packages/sema/src/backends/local-storage.ts`

```typescript
import type { VFSBackend, VFSHost } from "../vfs.js";

/** Options for LocalStorageBackend. */
export interface LocalStorageBackendOptions {
  /**
   * Namespace prefix for localStorage keys.
   * Each file is stored as `${namespace}:${path}`.
   * Directories are stored in a manifest key `${namespace}:__dirs__`.
   * @default "sema-vfs"
   */
  namespace?: string;
}

/**
 * VFS backend that persists files to localStorage.
 *
 * Simple and synchronous — good for small projects (< 5 MB).
 * localStorage has a ~5–10 MB limit per origin in most browsers.
 *
 * @example
 * ```ts
 * const sema = await SemaInterpreter.create({
 *   vfs: new LocalStorageBackend({ namespace: "my-project" }),
 * });
 * ```
 */
export class LocalStorageBackend implements VFSBackend {
  private ns: string;
  private filePrefix: string;
  private dirsKey: string;

  constructor(opts?: LocalStorageBackendOptions) {
    this.ns = opts?.namespace ?? "sema-vfs";
    this.filePrefix = this.ns + ":f:";
    this.dirsKey = this.ns + ":__dirs__";
  }

  async hydrate(host: VFSHost): Promise<void> {
    // Restore directories
    const dirsJson = localStorage.getItem(this.dirsKey);
    if (dirsJson) {
      try {
        const dirs: string[] = JSON.parse(dirsJson);
        for (const dir of dirs) {
          host.mkdir(dir);
        }
      } catch { /* ignore corrupt data */ }
    }

    // Restore files
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && key.startsWith(this.filePrefix)) {
        const path = key.slice(this.filePrefix.length);
        const content = localStorage.getItem(key);
        if (content !== null) {
          host.writeFile(path, content);
        }
      }
    }
  }

  async flush(host: VFSHost): Promise<void> {
    // Clear old entries for this namespace
    const toRemove: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && (key.startsWith(this.filePrefix) || key === this.dirsKey)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) {
      localStorage.removeItem(key);
    }

    // Write current files
    const allFiles = this.collectFiles(host, "/");
    for (const path of allFiles) {
      const content = host.readFile(path);
      if (content !== null) {
        localStorage.setItem(this.filePrefix + path, content);
      }
    }

    // Write directory manifest
    const dirs = this.collectDirs(host, "/");
    localStorage.setItem(this.dirsKey, JSON.stringify(dirs));
  }

  async reset(): Promise<void> {
    const toRemove: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && (key.startsWith(this.filePrefix) || key === this.dirsKey)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) {
      localStorage.removeItem(key);
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

**Step 2: Verify TypeScript compiles**

```bash
cd packages/sema && npx tsc --noEmit
```

**Step 3: Commit**

```bash
git add packages/sema/src/backends/local-storage.ts
git commit -m "feat(sema): add LocalStorageBackend for VFS persistence"
```

---

### Task 8: Wire VFSBackend into SemaInterpreter

**Files:**
- Modify: `packages/sema/src/index.ts`

**Step 1: Add imports and update InterpreterOptions**

At the top of the file, add:
```typescript
import type { VFSBackend, VFSHost } from "./vfs.js";
```

Add to `InterpreterOptions`:
```typescript
  /**
   * Optional VFS backend for persisting files across page reloads.
   * 
   * @example
   * ```js
   * import { SemaInterpreter, LocalStorageBackend } from "@sema-lang/sema";
   * const sema = await SemaInterpreter.create({
   *   vfs: new LocalStorageBackend({ namespace: "my-project" }),
   * });
   * ```
   */
  vfs?: VFSBackend;
```

**Step 2: Add backend storage and VFSHost bridge to SemaInterpreter class**

Add a private field:
```typescript
private _vfsBackend: VFSBackend | null;
```

Update the constructor:
```typescript
private constructor(inner: any, vfsBackend: VFSBackend | null) {
    this._inner = inner;
    this._vfsBackend = vfsBackend;
}
```

Add a private method that creates a VFSHost from the interpreter:
```typescript
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
```

**Step 3: Update `create()` to hydrate**

After creating the interpreter instance, add hydration:
```typescript
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

    // Hydrate VFS from backend if provided
    if (interp._vfsBackend) {
      await interp._vfsBackend.init?.();
      await interp._vfsBackend.hydrate(interp._vfsHost());
    }

    return interp;
}
```

**Step 4: Add `flushVFS()` method**

```typescript
  /**
   * Persist VFS changes to the configured backend.
   *
   * No-op if no VFS backend was provided during creation.
   * Call this after eval to save files, or set up periodic flushing.
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
```

**Step 5: Update `resetVFS()` to also reset the backend**

```typescript
  async resetVFS(): Promise<void> {
    this._inner.resetVFS();
    await this._vfsBackend?.reset?.();
  }
```

Wait — the existing `resetVFS` is synchronous. To avoid a breaking change, keep the sync version and add an async one:

```typescript
  /** Clear all files and directories from the in-memory VFS. */
  resetVFS(): void {
    this._inner.resetVFS();
  }

  /**
   * Clear the VFS and the persistent backend (if configured).
   */
  async resetVFSAndBackend(): Promise<void> {
    this._inner.resetVFS();
    await this._vfsBackend?.reset?.();
  }
```

**Step 6: Update exports in index.ts**

At the bottom, add re-exports:
```typescript
export type { VFSBackend, VFSHost } from "./vfs.js";
export { LocalStorageBackend } from "./backends/local-storage.js";
export type { LocalStorageBackendOptions } from "./backends/local-storage.js";
```

**Step 7: Verify TypeScript compiles**

```bash
cd packages/sema && npx tsc --noEmit
```

**Step 8: Commit**

```bash
git add packages/sema/src/index.ts
git commit -m "feat(sema): wire VFSBackend into SemaInterpreter with hydrate/flush lifecycle"
```

---

### Task 9: Rebuild WASM and run all tests

**Step 1: Rebuild WASM**

```bash
wasm-pack build crates/sema-wasm --target web --out-dir ../../playground/pkg
```

**Step 2: Run Playwright VFS demo test**

```bash
cd playground && npx playwright test tests/vfs-demo.spec.ts --reporter=line
```

**Step 3: Run all Playwright tests**

```bash
cd playground && npx playwright test --reporter=line
```

**Step 4: Run Rust tests**

```bash
cargo test
```

**Step 5: Run lints**

```bash
make lint
```

Expected: all pass. Fix any failures before committing.

**Step 6: Commit any fixes**

```bash
git commit -m "test: verify path normalization and VFS backend integration"
```

---

## Summary

| Task | What | Scope |
|------|------|-------|
| 1 | `normalize_path()` helper in Rust | S |
| 2 | Apply to all Sema-side `file/*` functions (13 fns) | M |
| 3 | Apply to all JS-side VFS methods (7 methods) | M |
| 4 | Simplify VFS demo `app.js` | S |
| 5 | Run integration tests | S |
| 6 | Define `VFSBackend` + `VFSHost` interfaces | S |
| 7 | Implement `LocalStorageBackend` | M |
| 8 | Wire backend into `SemaInterpreter.create()` | M |
| 9 | Full rebuild + test sweep | S |

Tasks 1–3 are sequential (each depends on the previous). Tasks 6–8 are sequential. Task 4 depends on 1–3. Task 5 depends on 1–4. Task 9 depends on all.
