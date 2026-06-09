# WASM Shims Design & Future Roadmap

> Design document for browser-based WASM shims in the Sema playground (`playground/crate/src/lib.rs`).

**Goal:** Provide meaningful implementations of stdlib functions that are gated behind `#[cfg(not(target_arch = "wasm32"))]` so playground users can run more examples without "Unbound variable" errors.

**Status:** HTTP via fetch bridge implemented — 2026-02-17

---

## What Was Implemented

### Tier 1: Trivial Shims (constants, no-ops, pass-throughs)

| Category             | Functions                                                                                                                                | Implementation                                          |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| **System constants** | `sys/platform`, `sys/arch`, `sys/os`                                                                                                     | Return `"web"`, `"wasm32"`, `"web"`                     |
| **System stubs**     | `sys/args`, `sys/cwd`, `sys/home-dir`, `sys/temp-dir`, `sys/hostname`, `sys/user`, `sys/pid`, `sys/which`, `sys/tty`, `sys/interactive?` | Sensible defaults (empty list, `"/"`, nil, etc.)        |
| **Timing**           | `sys/elapsed`                                                                                                                            | `Date.now()` delta from module load, converted to nanos |
| **No-ops**           | `sleep`, `env`, `sys/set-env`, `sys/env-all`                                                                                             | No-op or return nil/empty                               |
| **Errors**           | `exit`, `read-line`, `read-stdin`, `shell`                                                                                               | Return clear "not supported in WASM" errors             |
| **Terminal styling** | All 15 `term/*` color/modifier fns, `term/style`, `term/strip`, `term/rgb`                                                               | Pass-through (return text unchanged)                    |
| **Path operations**  | `path/join`, `path/dirname`, `path/basename`, `path/extension`, `path/absolute`                                                          | Pure string manipulation (no `std::path`)               |
| **IO/parsing**       | `read`, `read-many`, `error`                                                                                                             | Direct delegation to `sema_reader` / `SemaError`        |

### Tier 2: In-Memory Virtual Filesystem (VFS)

Thread-local `BTreeMap<String, String>` (files) + `BTreeSet<String>` (directories).

| Function                                                  | Behavior                                 |
| --------------------------------------------------------- | ---------------------------------------- |
| `file/read`, `file/write`, `file/append`                  | Read/write/append to VFS                 |
| `file/exists?`, `file/delete`, `file/rename`, `file/copy` | Standard file ops on VFS                 |
| `file/list`                                               | List direct children by prefix match     |
| `file/mkdir`                                              | Add path + all parents to directory set  |
| `file/is-directory?`, `file/is-file?`, `file/is-symlink?` | Check VFS state (symlink always false)   |
| `file/info`                                               | Map with `:size`, `:is-dir`, `:is-file`  |
| `file/read-lines`, `file/write-lines`                     | Line-oriented VFS operations             |
| `load`                                                    | Read from VFS + `sema_reader::read_many` |

**Limitation:** Session-only — all data lost on page reload.

### Tier 3: HTTP via Fetch Bridge ✅ (Implemented 2026-02-17)

`http/get`, `http/post`, `http/put`, `http/delete`, `http/request` work via browser `fetch()` API using a replay-with-cache strategy. WASM HTTP fns check an in-memory cache; on miss they raise a marker error caught by `eval_async`, which performs the actual `fetch()`, caches the response, and replays evaluation.

---

## Design Decisions

### D1: `sys/*` returns `"web"` not host OS detection

**Decision:** `sys/platform` → `"web"`, `sys/os` → `"web"`, `sys/arch` → `"wasm32"`

**Alternatives considered:**

- **(A) Parse `navigator.userAgent`** to detect macOS/Windows/Linux → Rejected. UA strings are increasingly unreliable (UA reduction, iPad masquerading, privacy browsers). Would be brittle and misleading since code runs in WASM sandbox, not natively.
- **(B) Use `navigator.platform`** → Rejected. Deprecated API.
- **(C) Use `navigator.userAgentData`** → Chromium-only, not portable.

**Rationale:** The code is running in a WASM sandbox. Reporting `"macos"` would be misleading — filesystem paths, processes, and signals don't exist. This matches how other languages handle it:

- Go: `GOOS=js`, `GOARCH=wasm`
- Rust: target is `wasm32-unknown-unknown`
- Python/Pyodide: `sys.platform` = `"emscripten"`

**Future:** Add `web/user-agent` as a WASM-only function returning the raw UA string for programs that need host hints (analytics, UI adaptation). Keep it separate from `sys/*` which describes the runtime target, not the host.

### D2: In-memory VFS over IndexedDB/OPFS for MVP

**Decision:** `BTreeMap<String, String>` + `BTreeSet<String>` in `thread_local!`

**Alternatives researched:**

| Approach              | Persistence      | Performance                   | Complexity                      | Browser Support                   |
| --------------------- | ---------------- | ----------------------------- | ------------------------------- | --------------------------------- |
| **In-memory HashMap** | Session only     | ✅ Fastest                    | ✅ Trivial                      | All                               |
| **IndexedDB**         | ✅ Cross-session | Slower (transaction overhead) | Medium (async sync)             | All                               |
| **OPFS**              | ✅ Cross-session | 10-100x faster than IDB       | High (Worker required for sync) | Chrome 86+, FF 111+, Safari 15.2+ |

**Rationale:** For a playground, session-only storage is acceptable. The VFS enables examples like turtle-svg, modules-demo, and streaming-io to run without requiring async bridges or Web Workers.

### D3: HTTP stubs (not async bridge) for MVP

**Decision:** Return clear error messages rather than implementing async HTTP.

**The fundamental problem:** Browser `fetch()` is async (Promise-based). The Sema evaluator is synchronous (`NativeFn` takes `&[Value]` → `Result<Value, SemaError>`). You cannot synchronously wait for a Promise on the main thread — it deadlocks the event loop.

**Approaches evaluated (ranked by feasibility):**

1. **`eval_async` + Promise/Future value** (Recommended next step)
   - Add `Value::Promise` variant (WASM-only, feature-gated)
   - `http/get` returns a Promise value; `(await p)` suspends evaluation
   - `eval_async` entry point drives the event loop
   - Effort: L (1-2 days)

2. **Suspend/resume effect system**
   - Evaluator step result becomes `Done(Value) | Suspend { op, continuation }`
   - `http/get` emits a Suspend effect; `eval_async` awaits it and resumes
   - Cleaner than embedding JS Promises in Value, more portable
   - Effort: L-XL (2-3 days)

3. **Worker + SharedArrayBuffer + Atomics.wait**
   - Run WASM interpreter in a Worker, use `Atomics.wait` to block
   - Requires cross-origin isolation headers (COOP/COEP)
   - Breaks on many hosting scenarios
   - Effort: XL (>2 days)

4. **Asyncify (wasm-opt --asyncify)**
   - Post-process WASM binary to allow pausing/resuming stack
   - Fragile with wasm-bindgen (designed for Emscripten)
   - Adds binary size and performance overhead
   - Effort: XL

**CORS reality:** Even with a working fetch bridge, cross-origin requests will frequently fail unless the target server sends `Access-Control-Allow-Origin: *`. A playground proxy endpoint would be needed for general HTTP access.

---

## Future Roadmap

### Phase 1: `web/*` namespace ✅ (Implemented)

WASM-only functions for browser environment detection:

- `web/user-agent` → raw `navigator.userAgent` string (all browsers)
- `web/user-agent-data` → structured map from `navigator.userAgentData` (Chromium-only, nil on Firefox/Safari)
  - Returns `{:mobile bool :platform "macOS" :brands ("Chromium/120" "Google Chrome/120")}` or nil

### Phase 2: HTTP via fetch ✅ (Implemented 2026-02-17)

Implemented via replay-with-cache strategy using `eval_async`/`eval_vm_async` + `web-sys` fetch.
Uses `wasm-bindgen-futures` and raw `web-sys` (not reqwest-wasm) to minimize binary size.
Returns same `{:status :headers :body}` map as native. CORS errors surface as `SemaError::Io`.

### Phase 3: OPFS-backed persistent VFS (Large, 2-3 days)

**Research findings on OPFS:**

- `FileSystemSyncAccessHandle` provides **synchronous file I/O** but only in Web Workers
- All modern browsers support it (Chrome 86+, Firefox 111+, Safari 15.2+)
- 10-100x faster than IndexedDB for file operations
- Storage quota: ~50% of free disk (Chrome), ~10% (Firefox), 1-2GB (Safari)
- Rust crate `anchpop/opfs` provides wasm-bindgen bindings

**Architecture for OPFS integration:**

```
┌─────────────────────┐
│  Main Thread (UI)   │
└──────────┬──────────┘
           │ postMessage
┌──────────▼──────────┐
│  Web Worker          │
│  WASM Interpreter    │
│  + SyncAccessHandle  │ ← synchronous file I/O
└──────────┬──────────┘
           │
┌──────────▼──────────┐
│  OPFS (Browser)      │
│  Persistent storage  │
└─────────────────────┘
```

**Requirements:** Moving the interpreter to a Web Worker (currently runs on main thread). This is a significant architectural change but also enables `Atomics.wait` for sync HTTP.

### Phase 4: Interpreter async primitives (XL, 3+ days)

If demand warrants, add structured concurrency to the interpreter:

- `Value::Task(id)` + task registry
- `spawn`, `join`, `await` special forms
- Internal event loop / scheduler
- Enables `(http/get url)` to "just work" in Lisp code

---

## Reference Implementations Studied

- **Pyodide** (Python): MEMFS default, IDBFS mount option, `pyfetch` async HTTP, deprecated sync XMLHttpRequest
- **Ruby WASM**: Uses `browser_wasi_shim` (MEMFS + OPFS support)
- **browser_wasi_shim**: Production MEMFS and OPFS implementations in TypeScript
- **happy-opfs**: Deno-like API with sync Worker support via SharedArrayBuffer
- **anchpop/opfs**: Rust crate with wasm-bindgen bindings for OPFS
