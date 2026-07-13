# Async Runtime Migration Inventory

This inventory tracks the hard-cut move to the interpreter-owned cooperative
runtime. A checked item means the production path uses runtime task suspension
or has been verified to be strictly synchronous and unable to call Sema.

## Runtime foundation

- [ ] Core task, wait, completion, cancellation, and task-local types
- [ ] VM task frames and 10,000-instruction quantum
- [ ] Interpreter root submission and host drive API
- [ ] FIFO ready queue, timer heap, completion inbox, shutdown/reaping
- [ ] Native `Return` / `Call` / `Suspend` outcomes and continuation frames
- [ ] Shared captured cells across task boundaries

## Language concurrency

- [ ] `async`, `await`, `async/spawn`, `async/run`, `async/await`
- [ ] `async/all`, `async/spawn-all`, `race`, `parallel`
- [ ] `async/map`, `async/pool-map`, higher-order async callbacks
- [ ] cancellation, cancellation observation, aggregate owned-child cleanup
- [ ] timers/sleep and virtual-clock tests
- [ ] channel create/send/receive/try/close/introspection

## Callback and context paths

- [ ] eval/call callback replacement in sema-core and sema-eval
- [ ] list/map/string/typed-array higher-order functions
- [ ] context, system, meta, workflow, OTel callbacks
- [ ] explicit module/file, sandbox, trace, usage, LLM, debugger task locals

## Async leaves and resources

- [ ] archive and PDF
- [ ] diff and git
- [ ] file I/O and shared I/O backend
- [ ] HTTP and WebSocket
- [ ] KV and SQLite
- [ ] process and PTY
- [ ] serial, secret, terminal, event
- [ ] streams, copy, close, and resource teardown
- [ ] HTTP server request dispatch and disconnect cancellation

## LLM and MCP

- [ ] completion/chat/compare/summarize/send/conversation async calls
- [ ] streaming, timeout, retry, cache, budget, usage, and tracing
- [ ] agent/tool callback loop
- [ ] MCP connect/list/call and per-connection queueing
- [ ] cancellation and late-result cleanup for every request family

## Hosts

- [ ] CLI expression/file/build and REPL
- [ ] embedded Rust API
- [ ] DAP and LSP evaluation
- [ ] notebook cells
- [ ] MCP server eval/build/notebook tools
- [ ] WASM Promise eval and playground worker/client

## Removal gates

- [ ] `IoHandle` / `IoPoll`
- [ ] `YieldReason` / scheduler target/run-result signals
- [ ] scheduler/evaluator thread-local callbacks
- [ ] `run_until_reentrant` and temporary running-task removal
- [ ] WASM replay marker, replay limit, synchronous XHR, and `Atomics.wait`
- [ ] subsystem-specific scheduler loops and blocking polls

## Verification

- [ ] scheduler characterization suite
- [ ] deterministic seeded scheduler stress suite
- [ ] native real-clock and I/O stress suite
- [ ] WASM browser async/debugger suite
- [ ] cancellation/leak/interpreter-drop suite
- [ ] static legacy-symbol inventory gate
- [ ] complete CI-equivalent release suite
