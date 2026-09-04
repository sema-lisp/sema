/* @ts-self-types="./sema_wasm.d.ts" */

export class SemaInterpreter {
    static __wrap(ptr) {
        const obj = Object.create(SemaInterpreter.prototype);
        obj.__wbg_ptr = ptr;
        SemaInterpreterFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SemaInterpreterFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_semainterpreter_free(ptr, 0);
    }
    /**
     * Request cancellation of the root whose id was reported by
     * [`Self::eval_promise`]'s `on_root_id` callback. Returns `false` if no
     * pending `evalPromise` root matches `root_id` (already settled, or
     * never existed) — a harmless no-op, same liveness contract as the
     * underlying `RuntimeCommandHandle::cancel_root`. It only accepts roots
     * registered with this Promise driver.
     * @param {number} root_id
     * @returns {boolean}
     */
    cancelRoot(root_id) {
        const ret = wasm.semainterpreter_cancelRoot(this.__wbg_ptr, root_id);
        return ret !== 0;
    }
    /**
     * Create interpreter with options: {stdlib: false, deny: ["network", "fs-write"]}
     * @param {any} opts
     * @returns {SemaInterpreter}
     */
    static createWithOptions(opts) {
        const ret = wasm.semainterpreter_createWithOptions(opts);
        return SemaInterpreter.__wrap(ret);
    }
    /**
     * @returns {any}
     */
    debugContinue() {
        const ret = wasm.semainterpreter_debugContinue(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<any>}
     */
    debugContinuePromise() {
        const ret = wasm.semainterpreter_debugContinuePromise(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {any}
     */
    debugGetLocals() {
        const ret = wasm.semainterpreter_debugGetLocals(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {any}
     */
    debugGetLocalsPromise() {
        const ret = wasm.semainterpreter_debugGetLocalsPromise(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {any}
     */
    debugGetStackTrace() {
        const ret = wasm.semainterpreter_debugGetStackTrace(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {any}
     */
    debugGetStackTracePromise() {
        const ret = wasm.semainterpreter_debugGetStackTracePromise(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {boolean}
     */
    debugIsActive() {
        const ret = wasm.semainterpreter_debugIsActive(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {boolean}
     */
    debugIsActivePromise() {
        const ret = wasm.semainterpreter_debugIsActivePromise(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {any}
     */
    debugPoll() {
        const ret = wasm.semainterpreter_debugPoll(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {Array<any>} lines
     */
    debugSetBreakpoints(lines) {
        wasm.semainterpreter_debugSetBreakpoints(this.__wbg_ptr, lines);
    }
    /**
     * @param {Array<any>} lines
     * @returns {boolean}
     */
    debugSetBreakpointsPromise(lines) {
        const ret = wasm.semainterpreter_debugSetBreakpointsPromise(this.__wbg_ptr, lines);
        return ret !== 0;
    }
    /**
     * Start a debug session. Compiles the code, sets breakpoints on given lines,
     * and runs until the first stop or completion.
     * Returns JSON: { status: "stopped"|"finished"|"error", ... }
     * @param {string} code
     * @param {Array<any>} breakpoint_lines
     * @returns {any}
     */
    debugStart(code, breakpoint_lines) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_debugStart(this.__wbg_ptr, ptr0, len0, breakpoint_lines);
        return ret;
    }
    /**
     * Start a per-interpreter, Promise-driven debug session. The returned
     * Promise settles only at a stable stop or terminal outcome; timer/HTTP
     * waits yield to the browser and resume the same runtime root.
     * @param {string} code
     * @param {Array<any>} breakpoint_lines
     * @returns {Promise<any>}
     */
    debugStartPromise(code, breakpoint_lines) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_debugStartPromise(this.__wbg_ptr, ptr0, len0, breakpoint_lines);
        return ret;
    }
    /**
     * @returns {any}
     */
    debugStepInto() {
        const ret = wasm.semainterpreter_debugStepInto(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<any>}
     */
    debugStepIntoPromise() {
        const ret = wasm.semainterpreter_debugStepIntoPromise(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {any}
     */
    debugStepOut() {
        const ret = wasm.semainterpreter_debugStepOut(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<any>}
     */
    debugStepOutPromise() {
        const ret = wasm.semainterpreter_debugStepOutPromise(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {any}
     */
    debugStepOver() {
        const ret = wasm.semainterpreter_debugStepOver(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<any>}
     */
    debugStepOverPromise() {
        const ret = wasm.semainterpreter_debugStepOverPromise(this.__wbg_ptr);
        return ret;
    }
    debugStop() {
        wasm.semainterpreter_debugStop(this.__wbg_ptr);
    }
    /**
     * @returns {boolean}
     */
    debugStopPromise() {
        const ret = wasm.semainterpreter_debugStopPromise(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Delete a file from the virtual filesystem. Returns true if the file existed.
     * @param {string} path
     * @returns {boolean}
     */
    deleteFile(path) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_deleteFile(this.__wbg_ptr, ptr0, len0);
        return ret !== 0;
    }
    /**
     * Snapshot the entire VFS as a plain JS object `{ files: {path: content},
     * dirs: [path] }` — structured-clonable across `postMessage`. Used by the
     * playground to mirror the worker's VFS back to the main thread after each
     * eval (and to seed the worker before one). See `loadVfs`.
     * @returns {any}
     */
    dumpVfs() {
        const ret = wasm.semainterpreter_dumpVfs(this.__wbg_ptr);
        return ret;
    }
    /**
     * Evaluate code, returns JSON: {"value": "...", "output": ["...", ...], "error": null}
     * or {"value": null, "output": [...], "error": "..."}
     * @param {string} code
     * @returns {any}
     */
    eval(code) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_eval(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Evaluate code with real (single-execution) async HTTP/sleep support in
     * the persistent global env (top-level defines persist across calls).
     *
     * A thin Promise-returning wrapper over [`Self::eval_promise`], kept as
     * its own entry point so existing JS callers
     * (`sema-web.js`, the playground's `?no-worker` fallback) don't have to
     * change; the program body is submitted as ONE root and never replayed.
     * See [`Self::eval_once_via_promise_seam`].
     * @param {string} code
     * @returns {Promise<any>}
     */
    evalAsync(code) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_evalAsync(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Evaluate in the global env so defines persist
     * @param {string} code
     * @returns {any}
     */
    evalGlobal(code) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_evalGlobal(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Evaluate `code` as ONE root on the unified runtime and return a
     * `Promise` that resolves with its printed value (or `null`) and rejects
     * with an `Error` on failure. The body runs once; `async/sleep` and
     * `http/get` suspend the root in place. Output is
     * NOT included in the resolved value: install `setPromiseOutputSink` to
     * receive this root's `println`/`print-err` output, tagged with its
     * root id, as it happens.
     *
     * `on_root_id`, if a function, is called SYNCHRONOUSLY (before this
     * method returns) with the new root's id as a JS `number` — the only way
     * a caller can learn it in time to route a later [`Self::cancel_root`]
     * call at the exact root this call submitted. The playground worker
     * protocol uses this to implement "Stop". Pass
     * `null`/`undefined` to skip.
     * @param {string} code
     * @param {any} on_root_id
     * @returns {Promise<any>}
     */
    evalPromise(code, on_root_id) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_evalPromise(this.__wbg_ptr, ptr0, len0, on_root_id);
        return ret;
    }
    /**
     * Evaluate code via the bytecode VM, returns same JSON format as eval_global
     * @param {string} code
     * @returns {any}
     */
    evalVM(code) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_evalVM(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Evaluate code with real (single-execution) async HTTP/sleep support
     * (bytecode VM). See [`Self::eval_async`] — identical wrapper; kept as a
     * separate name for the same JS-compat reason.
     * @param {string} code
     * @returns {Promise<any>}
     */
    evalVMAsync(code) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_evalVMAsync(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Check if a path exists in the virtual filesystem (file or directory).
     * @param {string} path
     * @returns {boolean}
     */
    fileExists(path) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_fileExists(this.__wbg_ptr, ptr0, len0);
        return ret !== 0;
    }
    /**
     * Compile code and return the set of lines that are valid breakpoint targets.
     * Returns a JS array of line numbers (sorted). Returns empty array on parse/compile error.
     * @param {string} code
     * @returns {Array<any>}
     */
    getValidBreakpointLines(code) {
        const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_getValidBreakpointLines(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Invoke a stored callback handle directly with JS arguments.
     * @param {number} callback_id
     * @param {Array<any>} args
     * @returns {any}
     */
    invokeCallback(callback_id, args) {
        const ret = wasm.semainterpreter_invokeCallback(this.__wbg_ptr, callback_id, args);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Invoke a named global function directly with JS arguments.
     *
     * This avoids reparsing source strings and works for functions
     * installed in the global environment.
     * @param {string} name
     * @param {Array<any>} args
     * @returns {any}
     */
    invokeGlobal(name, args) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_invokeGlobal(this.__wbg_ptr, ptr0, len0, args);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Check if a path is a directory in the virtual filesystem.
     * @param {string} path
     * @returns {boolean}
     */
    isDirectory(path) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_isDirectory(this.__wbg_ptr, ptr0, len0);
        return ret !== 0;
    }
    /**
     * List files and directories in the given directory path.
     * @param {string} dir
     * @returns {any}
     */
    listFiles(dir) {
        const ptr0 = passStringToWasm0(dir, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_listFiles(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Load a compiled web archive into the interpreter's embedded module table.
     * @param {Uint8Array} archive_bytes
     * @returns {any}
     */
    loadArchive(archive_bytes) {
        const ptr0 = passArray8ToWasm0(archive_bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_loadArchive(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Replace the entire VFS from a snapshot produced by `dumpVfs`. Resets
     * first, so the VFS exactly matches the snapshot.
     * @param {any} snapshot
     */
    loadVfs(snapshot) {
        wasm.semainterpreter_loadVfs(this.__wbg_ptr, snapshot);
    }
    /**
     * Create a directory in the virtual filesystem.
     * @param {string} path
     */
    mkdir(path) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.semainterpreter_mkdir(this.__wbg_ptr, ptr0, len0);
    }
    constructor() {
        const ret = wasm.semainterpreter_new();
        this.__wbg_ptr = ret;
        SemaInterpreterFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Inject a virtual module so that `(import "name")` resolves without a file.
     * @param {string} name
     * @param {string} source
     * @returns {any}
     */
    preloadModule(name, source) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(source, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_preloadModule(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * Read a file from the virtual filesystem.
     * @param {string} path
     * @returns {any}
     */
    readFile(path) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_readFile(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Register a JavaScript function callable from Sema code.
     * @param {string} name
     * @param {Function} callback
     */
    registerFunction(name, callback) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.semainterpreter_registerFunction(this.__wbg_ptr, ptr0, len0, callback);
    }
    /**
     * Release a callback handle that was materialized for JS.
     * @param {number} callback_id
     */
    releaseCallback(callback_id) {
        wasm.semainterpreter_releaseCallback(this.__wbg_ptr, callback_id);
    }
    /**
     * Clear all files and directories from the virtual filesystem.
     */
    resetVFS() {
        wasm.semainterpreter_resetVFS(this.__wbg_ptr);
    }
    /**
     * Execute an embedded archive entry path.
     * @param {string} path
     * @returns {any}
     */
    runEntry(path) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_runEntry(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Execute an embedded archive entry path with real (single-execution)
     * async HTTP/sleep support.
     *
     * Source-text and precompiled entries are each submitted as one root and
     * adopted by the same macrotask Promise driver. The program body never
     * replays, and `http/get`/`async/sleep` resume the original root in place.
     * @param {string} path
     * @returns {Promise<any>}
     */
    runEntryAsync(path) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_runEntryAsync(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Install a sink called with each completed output line as it is produced,
     * so the Web Worker can stream `println` output to the main thread live
     * (a long-running / sleeping program shows output as it happens). Pass a
     * JS function `(line: string) => void`.
     * @param {Function} sink
     */
    setOutputSink(sink) {
        wasm.semainterpreter_setOutputSink(this.__wbg_ptr, sink);
    }
    /**
     * Install (or clear, passing `null`/`undefined`) the JS callback that
     * receives `evalPromise` roots' output as `(rootId, stream, text)`,
     * where `stream` is `"stdout"` or `"stderr"`. Independent of
     * `setOutputSink` (the synchronous line-batched sink) — the two never
     * observe each other's output.
     * @param {any} sink
     */
    setPromiseOutputSink(sink) {
        wasm.semainterpreter_setPromiseOutputSink(this.__wbg_ptr, sink);
    }
    /**
     * Get the Sema version
     * @returns {string}
     */
    version() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.semainterpreter_version(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Get VFS usage statistics.
     * @returns {any}
     */
    vfsStats() {
        const ret = wasm.semainterpreter_vfsStats(this.__wbg_ptr);
        return ret;
    }
    /**
     * Write a file to the virtual filesystem.
     * @param {string} path
     * @param {string} content
     * @returns {any}
     */
    writeFile(path, content) {
        const ptr0 = passStringToWasm0(path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(content, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_writeFile(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
}
if (Symbol.dispose) SemaInterpreter.prototype[Symbol.dispose] = SemaInterpreter.prototype.free;

/**
 * Format Sema source code. Returns JSON: {"formatted": "...", "error": null}
 * or {"formatted": null, "error": "..."}
 * @param {string} code
 * @param {number} width
 * @param {number} indent
 * @param {boolean} align
 * @returns {any}
 */
export function formatCode(code, width, indent, align) {
    const ptr0 = passStringToWasm0(code, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.formatCode(ptr0, len0, width, indent, align);
    return ret;
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_boolean_get_c9c83ebd41b34df3: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_a57024b9c6e4a48b: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_is_function_5e4570eb24ffa122: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_7d13f41e1a2d5140: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_a2790eb24c211ea0: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_undefined_6cff064c44e0d823: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_number_get_136b9679cab35cfb: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_d154f1e671052120: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_bb96b2010945f0bc: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_be22cc64ae6946a0: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_abort_d8615b5857e112b3: function(arg0) {
            arg0.abort();
        },
        __wbg_apply_cb180996ed7fdae9: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.apply(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_0f2a9af232c18fd2: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.call(arg1, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_call_1c5886ab9c57d1c7: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_call_35dba3c747ad7521: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_39f824e18d9d2414: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.call(arg1, arg2, arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_close_716bcb607efb6fae: function(arg0) {
            arg0.close();
        },
        __wbg_done_669171204c3dcae2: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_eval_62d1ea2ebeca53ad: function() { return handleError(function (arg0, arg1) {
            const ret = eval(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_fetch_729fad2e5272298f: function(arg0, arg1) {
            const ret = arg0.fetch(arg1);
            return ret;
        },
        __wbg_fetch_d752d93f5b259503: function(arg0, arg1) {
            const ret = arg0.fetch(arg1);
            return ret;
        },
        __wbg_getRandomValues_436a51d0629d84e1: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getRandomValues_e446ea5ffdd14ee5: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getTime_63fb0332e6c4ec17: function(arg0) {
            const ret = arg0.getTime();
            return ret;
        },
        __wbg_get_971a0c45d172643f: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_c0c8f8d7da0c03dd: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_d173c0308df22d37: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_unchecked_e20b893aeafc3fca: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_headers_6dedf39f001ae99d: function(arg0) {
            const ret = arg0.headers;
            return ret;
        },
        __wbg_headers_92567b07014384b9: function(arg0) {
            const ret = arg0.headers;
            return ret;
        },
        __wbg_instanceof_ArrayBuffer_993d02d2d254cad1: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_8f49efbd4bfd76d6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_f935dbb0aa7cdeed: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_5625ff9937037a38: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_WorkerGlobalScope_8c58a6d74926b578: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WorkerGlobalScope;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_6339f732981044bf: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_iterator_5cebbb86e33c6dd6: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_keys_ec7f8c0c2370d91d: function(arg0) {
            const ret = Object.keys(arg0);
            return ret;
        },
        __wbg_length_36bd29c6848c2144: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_ecfa2c63d3d0d82c: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_new_0_f117d868b403dc07: function() {
            const ret = new Date();
            return ret;
        },
        __wbg_new_116be93542d39019: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_358857d90afd5a2d: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_418fb92a013d5930: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined_______true_(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_73118f90fa6698ff: function() { return handleError(function () {
            const ret = new MessageChannel();
            return ret;
        }, arguments); },
        __wbg_new_77cc4f4f472aeb81: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_ebe3e0f6837f0879: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_f5712de39c931ddf: function() { return handleError(function () {
            const ret = new AbortController();
            return ret;
        }, arguments); },
        __wbg_new_typed_cceaf62d8d95e9f2: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined_______true_(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_with_length_3ffc1c56427c525c: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_str_and_init_5a37d576dec75a86: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Request(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_next_42cf16ee0dafc9e2: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_8f26b64fa5e9f64b: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_now_8b265300afd5f2b9: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_now_e7c6795a7f81e10f: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_parse_1cc93481b0865939: function() { return handleError(function (arg0, arg1) {
            const ret = JSON.parse(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_performance_3fcf6e32a7e1ed0a: function(arg0) {
            const ret = arg0.performance;
            return ret;
        },
        __wbg_port1_1de7d907145688e5: function(arg0) {
            const ret = arg0.port1;
            return ret;
        },
        __wbg_port2_afd233d5a7a6fa07: function(arg0) {
            const ret = arg0.port2;
            return ret;
        },
        __wbg_postMessage_9d68c41311a76e69: function() { return handleError(function (arg0, arg1) {
            arg0.postMessage(arg1);
        }, arguments); },
        __wbg_prototypesetcall_de8e0d9553586985: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_push_adb0107829f02d75: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_queueMicrotask_ac694eae12e92dfb: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_queueMicrotask_be5fe34a8f4cad4d: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_resolve_020f95d838c6ef25: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_setTimeout_8be4960d8ad2bb76: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_setTimeout_eb1cd7bff8dee2a7: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_8155bb79a948541b: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_b9b5b5cb7b495037: function(arg0, arg1, arg2) {
            arg0.set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_body_f301b68bff45f419: function(arg0, arg1) {
            arg0.body = arg1;
        },
        __wbg_set_e92392c4b44c5de1: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.set(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_set_method_cf2b992b9a610bc3: function(arg0, arg1, arg2) {
            arg0.method = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_mode_d6479dfd6696c8d3: function(arg0, arg1) {
            arg0.mode = __wbindgen_enum_RequestMode[arg1];
        },
        __wbg_set_onmessage_dd565ea8943164ac: function(arg0, arg1) {
            arg0.onmessage = arg1;
        },
        __wbg_set_signal_115b9e9423652e66: function(arg0, arg1) {
            arg0.signal = arg1;
        },
        __wbg_signal_58449b7eb331d1be: function(arg0) {
            const ret = arg0.signal;
            return ret;
        },
        __wbg_static_accessor_GLOBAL_THIS_466428f93b4eaa76: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_c7aea38d4de089bc: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_42d4fae05e59267a: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_e0db14a0eba6a812: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_status_b0de02a07fd7d927: function(arg0) {
            const ret = arg0.status;
            return ret;
        },
        __wbg_stringify_f93a4ebae9231922: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(arg0);
            return ret;
        }, arguments); },
        __wbg_text_9302f33ea8cfce7b: function() { return handleError(function (arg0) {
            const ret = arg0.text();
            return ret;
        }, arguments); },
        __wbg_then_7026b513a94278a8: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_then_72819b8d4e081fb5: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_value_1e2369fab29b420e: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 64, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___wasm_bindgen_2465667bda35d78b___JsValue__core_f0fd674eaa06beef___result__Result_____wasm_bindgen_2465667bda35d78b___JsError___true_);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [F64, Externref, Externref], shim_idx: 18, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___f64__wasm_bindgen_2465667bda35d78b___JsValue__wasm_bindgen_2465667bda35d78b___JsValue______true_);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [F64], shim_idx: 16, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___f64______true_);
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 20, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_2465667bda35d78b___convert__closures_____invoke_______true_);
            return ret;
        },
        __wbindgen_cast_0000000000000005: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./sema_wasm_bg.js": import0,
    };
}

function wasm_bindgen_2465667bda35d78b___convert__closures_____invoke_______true_(arg0, arg1) {
    wasm.wasm_bindgen_2465667bda35d78b___convert__closures_____invoke_______true_(arg0, arg1);
}

function wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___wasm_bindgen_2465667bda35d78b___JsValue__core_f0fd674eaa06beef___result__Result_____wasm_bindgen_2465667bda35d78b___JsError___true_(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___wasm_bindgen_2465667bda35d78b___JsValue__core_f0fd674eaa06beef___result__Result_____wasm_bindgen_2465667bda35d78b___JsError___true_(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined_______true_(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined___js_sys_12ec15231502bf55___Function_fn_wasm_bindgen_2465667bda35d78b___JsValue_____wasm_bindgen_2465667bda35d78b___sys__Undefined_______true_(arg0, arg1, arg2, arg3);
}

function wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___f64______true_(arg0, arg1, arg2) {
    wasm.wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___f64______true_(arg0, arg1, arg2);
}

function wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___f64__wasm_bindgen_2465667bda35d78b___JsValue__wasm_bindgen_2465667bda35d78b___JsValue______true_(arg0, arg1, arg2, arg3, arg4) {
    wasm.wasm_bindgen_2465667bda35d78b___convert__closures_____invoke___f64__wasm_bindgen_2465667bda35d78b___JsValue__wasm_bindgen_2465667bda35d78b___JsValue______true_(arg0, arg1, arg2, arg3, arg4);
}


const __wbindgen_enum_RequestMode = ["same-origin", "no-cors", "cors", "navigate"];
const SemaInterpreterFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_semainterpreter_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => wasm.__wbindgen_destroy_closure(state.a, state.b));

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1 };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            wasm.__wbindgen_destroy_closure(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (!module.ok) {
            throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
        }

        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('sema_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
