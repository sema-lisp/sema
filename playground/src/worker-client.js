// Main-thread client for the Sema eval Web Worker — root-aware protocol v2
// (P6-3 step 3). Enables real wall-clock async/sleep and real (non-replayed)
// http/get by running eval on a worker that drives the Promise-based
// `evalPromise` seam (`crates/sema-wasm/src/driver.rs`, P6-3 step 2) instead
// of blocking on `Atomics.wait`. Gated: requires cross-origin isolation (kept
// as the availability signal, even though this protocol no longer allocates
// a SharedArrayBuffer itself — cross-origin isolation is what makes the
// worker eval path meaningfully different from the main thread, e.g. it
// stays responsive during a long computation) AND an explicit ?worker opt-in
// is not required (default-on); ?no-worker opts out to the main-thread
// fallback.

let worker = null;
let ready = null;
let nextId = 1;
let currentEvalId = null; // the eval message id currently in flight, for cancel
let outputHandler = null; // called with each streamed output line
const pending = new Map();

/** Register a handler called with each live output line during a worker run. */
export function setWorkerOutputHandler(fn) {
  outputHandler = fn;
}

/** True when the worker eval path should be used: the browser is cross-origin
 *  isolated (SharedArrayBuffer + Atomics available) and the user hasn't opted
 *  out with ?no-worker. Otherwise the playground runs on the main thread
 *  (instant virtual-clock sleeps), exactly as before. */
export function workerEvalEnabled() {
  return (
    typeof SharedArrayBuffer !== 'undefined' &&
    self.crossOriginIsolated === true &&
    !new URLSearchParams(location.search).has('no-worker')
  );
}

/** Spawn the worker and wait for it to load wasm. */
export function initWorker() {
  if (ready) return ready;
  worker = new Worker(new URL('dist/sema-worker.js', document.baseURI), { type: 'module' });
  worker.addEventListener('message', (e) => {
    const m = e.data;
    if (m && m.type === 'output') {
      if (outputHandler) outputHandler(m.text);
    } else if (m && m.type === 'result') {
      const resolve = pending.get(m.id);
      if (resolve) {
        pending.delete(m.id);
        resolve({ result: m.result, vfs: m.vfs });
      }
    }
  });
  ready = new Promise((res) => {
    const onReady = (e) => {
      if (e.data && e.data.type === 'ready') {
        worker.removeEventListener('message', onReady);
        res();
      }
    };
    worker.addEventListener('message', onReady);
    worker.postMessage({ type: 'init' });
  });
  return ready;
}

/** Request cancellation of the currently running eval: sends a `cancel`
 *  message tagged with its eval id, which the worker maps to the exact root
 *  `evalPromise` reported for it and cancels via `cancelRoot` (design doc
 *  §2.4). The worker survives (defines + VFS preserved). */
export function cancelWorker() {
  if (!worker || currentEvalId === null) return;
  worker.postMessage({ type: 'cancel', id: currentEvalId });
}

/** Evaluate `code` on the worker, seeding it with `vfs` (a dumpVfs snapshot).
 *  Resolves to { result: {value,output,error}, vfs: snapshot-after-run }. */
export async function evalViaWorker(code, vfs) {
  await ready;
  const id = nextId++;
  currentEvalId = id;
  try {
    return await new Promise((resolve) => {
      pending.set(id, resolve);
      worker.postMessage({ type: 'eval', id, code, vfs });
    });
  } finally {
    if (currentEvalId === id) currentEvalId = null;
  }
}
