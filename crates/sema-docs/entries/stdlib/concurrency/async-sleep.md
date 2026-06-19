---
name: "async/sleep"
module: "concurrency"
section: "Promises"
---

```sema
(async/sleep ms)
```

Inside an async task, yield for `ms` milliseconds on the scheduler's virtual clock. The clock only advances when every task is blocked, jumping to the nearest deadline, so a shorter sleep always wakes before a longer one, deterministically. The scheduler then waits the real time when it advances: on native via `thread::sleep`, and in the browser playground by running eval on a Web Worker that blocks on `Atomics.wait` (so a sleep really pauses while the page stays responsive). Browsers without cross-origin isolation fall back to advancing the clock instantly (ordering preserved). Outside async, calls `thread::sleep` on native. Durations are capped at `86_400_000` ms (1 day).
