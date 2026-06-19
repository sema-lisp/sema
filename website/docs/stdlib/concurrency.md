---
outline: [2, 3]
---

# Concurrency

Cooperative async concurrency with promises and channels. Tasks run on the VM's cooperative scheduler, interleaving at yield points (channel operations, `await`, `sleep`).

## Scheduling guarantees

- **Spawn order is preserved.** When several tasks are simultaneously ready to run, the scheduler picks them in the order they were spawned. A pipeline of `(async (send-1)) (async (send-2)) (async (send-3))` followed by sequential receives yields `1 2 3`, not a reordered surface.
- **Wake order is FIFO.** When a value becomes available on a channel, the longest-waiting receiver is woken first.
- **Cooperation, not parallelism.** Tasks interleave at yield points (channel ops, `await`, `sleep`). CPU-bound tasks without yield points run to completion before other tasks get a turn.

## Promises

### `async/spawn`

```sema
(async/spawn thunk) → async-promise
```

Spawn a zero-argument function as an async task. Returns a promise that resolves when the task completes.

```sema
(define p (async/spawn (fn () (+ 1 2))))
(async/await p)  ; => 3
```

Usually called via the `async` special form:

```sema
(define p (async (+ 1 2)))
(await p)  ; => 3
```

### `async/await`

```sema
(async/await promise) → value
```

Wait for a promise to resolve. Inside an async task, yields to the scheduler. At the top level, runs the scheduler inline until the promise resolves. Raises an error if the promise was rejected.

### `async/all`

```sema
(async/all promises) → list
```

Run all promises to completion and return a list of their results. Takes a list or vector of promises.

```sema
(let ((p1 (async 10))
      (p2 (async 20))
      (p3 (async 30)))
  (async/all (list p1 p2 p3)))  ; => (10 20 30)
```

### `async/race`

```sema
(async/race promises) → value
```

Return the value of the first promise to resolve. Takes a list or vector of promises.

### `async/resolved`

```sema
(async/resolved value) → async-promise
```

Create an already-resolved promise wrapping `value`.

### `async/rejected`

```sema
(async/rejected message) → async-promise
```

Create an already-rejected promise with `message`.

### `async/run`

```sema
(async/run)
```

Run all pending async tasks to completion.

### `async/sleep`

```sema
(async/sleep ms)
```

Inside an async task, yield for `ms` milliseconds on the scheduler's **virtual clock**. The clock only advances when every task is blocked, jumping to the nearest deadline — so a shorter sleep always wakes before a longer one, deterministically. The scheduler then waits the real time when it advances: on native via `thread::sleep`, and in the **browser playground** by running eval on a Web Worker that blocks on `Atomics.wait` (so a sleep really pauses while the page stays responsive). Browsers without cross-origin isolation fall back to advancing the clock instantly — durations still order tasks correctly, just without the real wait. Outside async, calls `thread::sleep` on native. Durations are capped at `86_400_000` ms (1 day).

### `async/timeout`

```sema
(async/timeout ms promise) → value
```

Wait for `promise` to resolve, but raise an error if it takes longer than `ms` milliseconds. The underlying task is **not** automatically cancelled; pair with `async/cancel` if you need to free its resources.

```sema
(async/timeout 100 (async (do-slow-work)))
;; raises: async/timeout: operation timed out
```

A `ms = 0` (or very short) timeout still lets work that is **synchronously ready** finish — it only fires once the virtual clock actually reaches the deadline with the task still pending (i.e. the task had to block/wait). Durations are capped at `86_400_000` ms (1 day).

### `async/cancel`

```sema
(async/cancel promise) → bool
```

Request cancellation of a spawned task. Returns `#t` if the call actually transitioned the promise into the `Cancelled` state, `#f` if there was nothing to cancel — the promise was already terminal (resolved, rejected, previously cancelled) or was never spawned in the first place (e.g. created via `async/resolved`).

Cancellation is best-effort and never errors. The next time the task hits a yield point, it transitions to `Cancelled`; subsequent `(await p)` raises `"async/await: task was cancelled"` (distinct from a normal rejection).

```sema
(async/cancel (async/resolved 1))                ;; => #f  (never spawned)
(let ((p (async 42))) (await p) (async/cancel p)) ;; => #f  (already resolved)
(let ((p (async (async/sleep 100)))) (async/cancel p)) ;; => #t
```

### `async/cancelled?`

```sema
(async/cancelled? promise) → bool
```

`#t` if `promise` is in the `Cancelled` state — distinct from `async/rejected?`. Matches the state variant directly rather than the rejection message, so a user `(async/rejected "cancelled")` no longer aliases:

```sema
(async/cancelled? (async/rejected "cancelled"))  ;; => #f
```

### Promise predicates

The four predicates **partition** the terminal states: a promise is at most one of resolved / rejected / cancelled, and `pending?` is the complement of those three.

| Function | Description |
| --- | --- |
| `(async/promise? x)` | Is `x` an async promise? |
| `(async/resolved? p)` | Is promise `p` resolved? |
| `(async/rejected? p)` | Is promise `p` rejected? (excludes cancelled) |
| `(async/pending? p)` | Is promise `p` still pending? |
| `(async/cancelled? p)` | Was promise `p` cancelled? |

## Channels

Bounded FIFO channels for communication between async tasks.

### `channel/new`

```sema
(channel/new)         → channel  ; capacity 1
(channel/new capacity) → channel
```

Create a bounded channel. Default capacity is 1. Capacity must be at least 1.

### `channel/send`

```sema
(channel/send ch value)
```

Send a value to the channel. If the channel is full and inside an async task, yields until space is available. Outside async context, raises an error if full. Raises an error if the channel is closed.

### `channel/recv`

```sema
(channel/recv ch) → value
```

Receive a value from the channel. If the channel is empty and inside an async task, yields until data is available. Outside async context, raises an error if empty. Returns `nil` if the channel is closed and empty.

### `channel/try-recv`

```sema
(channel/try-recv ch) → value | nil
```

Non-blocking receive. Returns the next value or `nil` if the channel is empty.

### `channel/close`

```sema
(channel/close ch)
```

Close the channel. Subsequent sends will error. Blocked receivers will wake with `nil`.

### Channel predicates

| Function | Description |
| --- | --- |
| `(channel? x)` | Is `x` a channel? |
| `(channel/closed? ch)` | Is the channel closed? |
| `(channel/empty? ch)` | Is the channel buffer empty? |
| `(channel/full? ch)` | Is the channel buffer at capacity? |
| `(channel/count ch)` | Number of values in the buffer |

## Examples

### Producer/Consumer

```sema
(let ((ch (channel/new 1)))
  (let ((producer (async
          (channel/send ch 10)
          (channel/send ch 20)
          (channel/send ch 30)
          (channel/close ch)))
        (consumer (async
          (let loop ((sum 0))
            (let ((val (channel/recv ch)))
              (if (nil? val)
                sum
                (loop (+ sum val))))))))
    (await consumer)))  ; => 60
```

### Parallel computation

```sema
(let ((p1 (async (fib 30)))
      (p2 (async (fib 31))))
  (+ (await p1) (await p2)))
```

See [Scheduling guarantees](#scheduling-guarantees) above for the full ordering / cooperation rules.

## Async ops inside higher-order functions

Stdlib higher-order functions like `for-each`, `map`, `filter`, `foldl`, `sort-by`, `apply`, `reduce`, `partition`, `any`, `every` can call **lambdas** that perform async operations (`channel/send`, `channel/recv`, `await`, `async/sleep`). The yield suspends inside the callback and resumes correctly:

```sema
(let ((ch (channel/new 3)))
  (let ((producer (async
                    (for-each (fn (n) (channel/send ch n))
                              (list 1 2 3 4 5 6 7))
                    (channel/close ch)))
        (consumer (async
                    (let loop ((sum 0))
                      (let ((v (channel/recv ch)))
                        (if (nil? v) sum (loop (+ sum v))))))))
    (await consumer)))   ;; => 28
```

Yielding **native** functions (e.g., `channel/recv`, `async/sleep`) passed *directly* as the callback produce a clear error pointing to the workaround:

```sema
;; Error: yielding native passed directly to a higher-order function — wrap in a lambda
(map channel/recv (list ch ch ch))

;; Correct: wrap the native in a lambda
(map (fn (c) (channel/recv c)) (list ch ch ch))
```
