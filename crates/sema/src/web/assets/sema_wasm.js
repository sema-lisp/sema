/* @ts-self-types="./sema_wasm.d.ts" */

export class SemaInterpreter {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
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
     * @returns {any}
     */
    debugGetLocals() {
        const ret = wasm.semainterpreter_debugGetLocals(this.__wbg_ptr);
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
     * @returns {boolean}
     */
    debugIsActive() {
        const ret = wasm.semainterpreter_debugIsActive(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Perform an HTTP fetch from a debug marker and cache the result.
     * Called by JS in response to a "http_needed" status.
     * Takes the marker JSON from the request field. Returns true on success.
     * @param {string} marker_json
     * @returns {Promise<boolean>}
     */
    debugPerformFetch(marker_json) {
        const ptr0 = passStringToWasm0(marker_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.semainterpreter_debugPerformFetch(this.__wbg_ptr, ptr0, len0);
        return ret;
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
     * Start a debug session. Compiles the code, sets breakpoints on given lines,
     * and runs until the first stop or completion.
     * Returns JSON: { status: "stopped"|"finished"|"error"|"http_needed", ... }
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
     * @returns {any}
     */
    debugStepInto() {
        const ret = wasm.semainterpreter_debugStepInto(this.__wbg_ptr);
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
     * @returns {any}
     */
    debugStepOver() {
        const ret = wasm.semainterpreter_debugStepOver(this.__wbg_ptr);
        return ret;
    }
    debugStop() {
        wasm.semainterpreter_debugStop(this.__wbg_ptr);
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
     * Evaluate code with async HTTP support in the persistent global env
     * (top-level defines persist across calls). Runs on the bytecode VM.
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
     * Evaluate code with async HTTP support (bytecode VM)
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
     * Enable real wall-clock `async/sleep` via `Atomics.wait` on the given
     * control buffer. Call this once from a Web Worker (where blocking is
     * allowed), passing an `Int32Array` over a `SharedArrayBuffer` shared with
     * the main thread. After this, the scheduler's virtual-clock advances also
     * block the worker for the real duration. Do NOT call on the main thread —
     * `Atomics.wait` is illegal there; leaving it uninstalled keeps the
     * instant virtual-clock behavior.
     * @param {Int32Array} view
     */
    installAtomicsSleep(view) {
        wasm.semainterpreter_installAtomicsSleep(this.__wbg_ptr, view);
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
        this.__wbg_ptr = ret >>> 0;
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
     * Execute an embedded archive entry path with async HTTP replay support.
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
        __wbg___wbindgen_boolean_get_c0f3f60bac5a78d1: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_5398f5bb970e0daa: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_is_function_3c846841762788c1: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_0b605fc6b167c56f: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_781bc9f159099513: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_undefined_52709e72fb9f179c: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_number_get_34bb9d9dcfa21373: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_395e606bd0ee4427: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_6ddd609b62940d55: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_6b5b6b8576d35cb1: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_abort_5ef96933660780b7: function(arg0) {
            arg0.abort();
        },
        __wbg_apply_ac9afb97ca32f169: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.apply(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_2d781c1f4d5c0ef8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_e133b57c9155d22c: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_done_08ce71ee07e3bd17: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_eval_c311194bb27c7836: function() { return handleError(function (arg0, arg1) {
            const ret = eval(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_fetch_f8a611684c3b5fe5: function(arg0, arg1) {
            const ret = arg0.fetch(arg1);
            return ret;
        },
        __wbg_getAllResponseHeaders_0d155233eff8d5a4: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.getAllResponseHeaders();
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_getRandomValues_76dfc69825c9c552: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getRandomValues_a1cf2e70b003a59d: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getTime_1dad7b5386ddd2d9: function(arg0) {
            const ret = arg0.getTime();
            return ret;
        },
        __wbg_get_326e41e095fb2575: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_3ef1eba1850ade27: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_a8ee5c45dabc1b3b: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_unchecked_329cfe50afab7352: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_headers_eb2234545f9ff993: function(arg0) {
            const ret = arg0.headers;
            return ret;
        },
        __wbg_headers_fc8c672cd757e0fd: function(arg0) {
            const ret = arg0.headers;
            return ret;
        },
        __wbg_instanceof_ArrayBuffer_101e2bf31071a9f6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_9b4d9fd451e051b1: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_740438561a5b956d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_23e677d2c6843922: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_33b91feb269ff46e: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_iterator_d8f549ec8fb061b1: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_keys_ab0d051a1c55236d: function(arg0) {
            const ret = Object.keys(arg0);
            return ret;
        },
        __wbg_length_b3416cf66a5452c8: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_ea16607d7b61445b: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_load_d8bce92127bf3f7d: function() { return handleError(function (arg0, arg1) {
            const ret = Atomics.load(arg0, arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_new_0_1dcafdf5e786e876: function() {
            const ret = new Date();
            return ret;
        },
        __wbg_new_5f486cdf45a04d78: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_a70fbab9066b301f: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_ab79df5bd7c26067: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_c518c60af666645b: function() { return handleError(function () {
            const ret = new AbortController();
            return ret;
        }, arguments); },
        __wbg_new_cb1d07f18f0aae72: function() { return handleError(function () {
            const ret = new XMLHttpRequest();
            return ret;
        }, arguments); },
        __wbg_new_typed_aaaeaf29cf802876: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen_99a98757d426b094___convert__closures_____invoke___js_sys_82c2e4c9bb939c97___Function_fn_wasm_bindgen_99a98757d426b094___JsValue_____wasm_bindgen_99a98757d426b094___sys__Undefined___js_sys_82c2e4c9bb939c97___Function_fn_wasm_bindgen_99a98757d426b094___JsValue_____wasm_bindgen_99a98757d426b094___sys__Undefined_______true_(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = state0.b = 0;
            }
        },
        __wbg_new_with_length_825018a1616e9e55: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_str_and_init_b4b54d1a819bc724: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Request(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_next_11b99ee6237339e3: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_e01a967809d1aa68: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_now_16f0c993d5dd6c27: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_open_ab5f9641f561c051: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), arg5 !== 0);
        }, arguments); },
        __wbg_parse_e9eddd2a82c706eb: function() { return handleError(function (arg0, arg1) {
            const ret = JSON.parse(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_prototypesetcall_d62e5099504357e6: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_push_e87b0e732085a946: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_queueMicrotask_0c399741342fb10f: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_a082d78ce798393e: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_resolve_ae8d83246e5bcc12: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_responseText_3ee457c31fe90e0e: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.responseText;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_send_442fe07c698a9f29: function() { return handleError(function (arg0) {
            arg0.send();
        }, arguments); },
        __wbg_send_7aad46f9e0f7ecca: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.send(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_setRequestHeader_4d392f8eb9f8a78b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setRequestHeader(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setTimeout_7f7035ad0b026458: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_7eaa4f96924fd6b3: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_8c0b3ffcf05d61c2: function(arg0, arg1, arg2) {
            arg0.set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_body_a3d856b097dfda04: function(arg0, arg1) {
            arg0.body = arg1;
        },
        __wbg_set_e09648bea3f1af1e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.set(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_set_method_8c015e8bcafd7be1: function(arg0, arg1, arg2) {
            arg0.method = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_mode_5a87f2c809cf37c2: function(arg0, arg1) {
            arg0.mode = __wbindgen_enum_RequestMode[arg1];
        },
        __wbg_set_signal_0cebecb698f25d21: function(arg0, arg1) {
            arg0.signal = arg1;
        },
        __wbg_signal_166e1da31adcac18: function(arg0) {
            const ret = arg0.signal;
            return ret;
        },
        __wbg_static_accessor_GLOBAL_8adb955bd33fac2f: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_ad356e0db91c7913: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_f207c857566db248: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_bb9f1ba69d61b386: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_status_318629ab93a22955: function(arg0) {
            const ret = arg0.status;
            return ret;
        },
        __wbg_status_d5251b0ac97c56d5: function() { return handleError(function (arg0) {
            const ret = arg0.status;
            return ret;
        }, arguments); },
        __wbg_stringify_5ae93966a84901ac: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(arg0);
            return ret;
        }, arguments); },
        __wbg_text_372f5b91442c50f9: function() { return handleError(function (arg0) {
            const ret = arg0.text();
            return ret;
        }, arguments); },
        __wbg_then_098abe61755d12f6: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_then_9e335f6dd892bc11: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_value_21fc78aab0322612: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_wait_764625a35886f35b: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = Atomics.wait(arg0, arg1 >>> 0, arg2, arg3);
            return ret;
        }, arguments); },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 1, function: Function { arguments: [Externref], shim_idx: 45, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen_99a98757d426b094___closure__destroy___dyn_core_7d5f0a2ba6a62c33___ops__function__FnMut__wasm_bindgen_99a98757d426b094___JsValue____Output___core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_99a98757d426b094___JsError___, wasm_bindgen_99a98757d426b094___convert__closures_____invoke___wasm_bindgen_99a98757d426b094___JsValue__core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_99a98757d426b094___JsError___true_);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 1, function: Function { arguments: [], shim_idx: 2, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen_99a98757d426b094___closure__destroy___dyn_core_7d5f0a2ba6a62c33___ops__function__FnMut__wasm_bindgen_99a98757d426b094___JsValue____Output___core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_99a98757d426b094___JsError___, wasm_bindgen_99a98757d426b094___convert__closures_____invoke_______true_);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
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

function wasm_bindgen_99a98757d426b094___convert__closures_____invoke_______true_(arg0, arg1) {
    wasm.wasm_bindgen_99a98757d426b094___convert__closures_____invoke_______true_(arg0, arg1);
}

function wasm_bindgen_99a98757d426b094___convert__closures_____invoke___wasm_bindgen_99a98757d426b094___JsValue__core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_99a98757d426b094___JsError___true_(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_99a98757d426b094___convert__closures_____invoke___wasm_bindgen_99a98757d426b094___JsValue__core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_99a98757d426b094___JsError___true_(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_99a98757d426b094___convert__closures_____invoke___js_sys_82c2e4c9bb939c97___Function_fn_wasm_bindgen_99a98757d426b094___JsValue_____wasm_bindgen_99a98757d426b094___sys__Undefined___js_sys_82c2e4c9bb939c97___Function_fn_wasm_bindgen_99a98757d426b094___JsValue_____wasm_bindgen_99a98757d426b094___sys__Undefined_______true_(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen_99a98757d426b094___convert__closures_____invoke___js_sys_82c2e4c9bb939c97___Function_fn_wasm_bindgen_99a98757d426b094___JsValue_____wasm_bindgen_99a98757d426b094___sys__Undefined___js_sys_82c2e4c9bb939c97___Function_fn_wasm_bindgen_99a98757d426b094___JsValue_____wasm_bindgen_99a98757d426b094___sys__Undefined_______true_(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_RequestMode = ["same-origin", "no-cors", "cors", "navigate"];
const SemaInterpreterFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_semainterpreter_free(ptr >>> 0, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => state.dtor(state.a, state.b));

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
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
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

function makeMutClosure(arg0, arg1, dtor, f) {
    const state = { a: arg0, b: arg1, cnt: 1, dtor };
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
            state.dtor(state.a, state.b);
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

let wasmModule, wasm;
function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

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
