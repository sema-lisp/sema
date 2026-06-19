// M1 spike worker: block the worker thread for ~1000ms using Atomics.wait on a
// shared Int32Array. The value at index 0 stays 0, so the wait times out after
// the timeout — proving a worker can do a *real* blocking sleep. This is the
// exact primitive the scheduler's wasm sleep-arm would call (M2).
self.onmessage = (e) => {
  const i32 = new Int32Array(e.data.sab);
  const t0 = performance.now();
  const res = Atomics.wait(i32, 0, 0, 1000); // -> "timed-out" after ~1000ms
  self.postMessage({ res, elapsed: performance.now() - t0 });
};
