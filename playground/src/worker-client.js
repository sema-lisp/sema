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
let readyResolve = null;
let readyReject = null;
let initTimer = null;

const WORKER_INIT_TIMEOUT_MS = 10_000;

function asError(value, fallback) {
  if (value instanceof Error) return value;
  if (typeof value === 'string' && value) return new Error(value);
  return new Error(fallback);
}

function clearInitState() {
  if (initTimer !== null) clearTimeout(initTimer);
  initTimer = null;
  readyResolve = null;
  readyReject = null;
}

function failWorker(reason) {
  const error = asError(reason, 'The Sema evaluation worker failed.');
  const rejectReady = readyReject;
  clearInitState();
  rejectReady?.(error);

  for (const request of pending.values()) request.reject(error);
  pending.clear();
  currentEvalId = null;

  worker?.terminate();
  worker = null;
  ready = null;
}

/** Register a handler called with each live output line during a worker run. */
export function setWorkerOutputHandler(fn) {
  outputHandler = fn;
}

/** True when the worker eval path should be used: the browser is cross-origin
 *  isolated (with SharedArrayBuffer available) and the user hasn't opted
 *  out with ?no-worker. Otherwise the same Promise runtime runs on the main
 *  thread with real timers and HTTP, but CPU-bound work shares the UI thread. */
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
      const request = pending.get(m.id);
      if (request) {
        pending.delete(m.id);
        request.resolve({ result: m.result, vfs: m.vfs });
      }
    } else if (m && m.type === 'init_error') {
      failWorker(m.error || 'The Sema evaluation worker could not initialize.');
    }
  });
  worker.addEventListener('error', (event) => {
    event.preventDefault();
    failWorker(event.message || 'The Sema evaluation worker crashed.');
  });
  worker.addEventListener('messageerror', () => {
    failWorker('The Sema evaluation worker sent an unreadable message.');
  });
  ready = new Promise((resolve, reject) => {
    readyResolve = resolve;
    readyReject = reject;
    initTimer = setTimeout(() => {
      failWorker(`The Sema evaluation worker did not initialize within ${WORKER_INIT_TIMEOUT_MS}ms.`);
    }, WORKER_INIT_TIMEOUT_MS);
    const onReady = (event) => {
      if (event.data && event.data.type === 'ready') {
        worker?.removeEventListener('message', onReady);
        const resolveReady = readyResolve;
        clearInitState();
        resolveReady?.();
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
  if (!ready) throw new Error('The Sema evaluation worker is not initialized.');
  await ready;
  if (!worker) throw new Error('The Sema evaluation worker is unavailable.');
  const id = nextId++;
  currentEvalId = id;
  try {
    return await new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      worker.postMessage({ type: 'eval', id, code, vfs });
    });
  } finally {
    if (currentEvalId === id) currentEvalId = null;
  }
}
