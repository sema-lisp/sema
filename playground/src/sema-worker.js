// Sema eval Web Worker — root-aware protocol v2 (P6-3 step 3).
//
// Runs the WASM VM off the main UI thread. `evalPromise` (the Promise-driven
// eval seam, P6-3 step 2 — `crates/sema-wasm/src/driver.rs`) submits the
// program as ONE root on the unified runtime and settles it via a macrotask
// driver; `async/sleep` and `http/get` complete through real `setTimeout`/
// `fetch` callbacks, so a real wall-clock sleep no longer needs to block this
// thread on `Atomics.wait` — the worker stays free to pump its own macrotask
// queue and to receive a `cancel` message mid-run. Message protocol:
//   main -> { type:'init' }
//   worker -> { type:'ready' }
//   main -> { type:'eval', id, code, vfs }
//   worker -> { type:'output', id, rootId, stream, text }   (streamed live)
//   worker -> { type:'result', id, result:{value,output,error}, vfs }
//   main -> { type:'cancel', id }   Stop: cancels root `id`'s in-flight eval
//
// The control `SharedArrayBuffer`/`installAtomicsSleep`/`Atomics.wait` replay
// path is gone (P6-3 step 5 — `docs/plans/2026-07-16-wasm-promise-driven-roots.md`):
// this worker never allocates a SAB and cancellation routes exclusively
// through `cancelRoot`.
import init, { SemaInterpreter } from '../pkg/sema_wasm.js';

let interp = null;
// eval message id -> the root id `evalPromise` reported for it, so a later
// `cancel` message (keyed by the same eval id) can route to the exact root.
const activeRoots = new Map();

self.onmessage = async (e) => {
  const msg = e.data;
  try {
    if (msg.type === 'init') {
      await init();
      interp = new SemaInterpreter();
      // Root-tagged output: forward every evalPromise root's println/print
      // output to the main thread as it happens, tagged with the eval
      // message id it belongs to (looked up from its root id) so the main
      // thread never needs to know raw root ids.
      interp.setPromiseOutputSink((rootId, stream, text) => {
        let evalId;
        for (const [id, root] of activeRoots) {
          if (root === rootId) {
            evalId = id;
            break;
          }
        }
        self.postMessage({ type: 'output', id: evalId, rootId, stream, text });
      });
      self.postMessage({ type: 'ready' });
      return;
    }
    if (msg.type === 'eval') {
      // Seed the worker's VFS from the main thread's mirror, run, then return
      // the resulting VFS so the main thread can reflect any file changes.
      if (msg.vfs !== undefined) interp.loadVfs(msg.vfs);
      let value = null;
      let error = null;
      try {
        value = await new Promise((resolve, reject) => {
          const promise = interp.evalPromise(msg.code, (rootId) => {
            activeRoots.set(msg.id, rootId);
          });
          promise.then(resolve, reject);
        });
      } catch (err) {
        error = (err && err.message) ? err.message : String(err);
      } finally {
        activeRoots.delete(msg.id);
      }
      const vfs = interp.dumpVfs();
      self.postMessage({
        type: 'result',
        id: msg.id,
        result: { value, output: [], error },
        vfs,
      });
      return;
    }
    if (msg.type === 'cancel') {
      const rootId = activeRoots.get(msg.id);
      if (rootId !== undefined) interp.cancelRoot(rootId);
      return;
    }
  } catch (err) {
    const message = (err && err.message) ? err.message : String(err);
    self.postMessage({
      type: 'result',
      id: msg && msg.id,
      result: { value: null, output: [], error: message },
    });
  }
};
