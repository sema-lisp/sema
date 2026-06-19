---
outline: [2, 3]
---

# Concurrency & Async

Sema features a **cooperative asynchronous concurrency model** using Promises and Channels. Tasks run on the bytecode VM's built-in scheduler and interleave execution at specific **yield points** (such as waiting for a channel, sleeping, or awaiting another task).

> [!IMPORTANT]
> Async features are **VM-only** and require the bytecode VM backend (which is the default since v1.13).

---

## 1. Promises and Tasks

A Promise represents the result of a computation that runs asynchronously.

### Spawning Tasks (`async`)
You can spawn a computation as a background task using the `async` special form, which returns an async promise:

```sema
(define p (async (+ 10 20)))
```

### Awaiting Results (`await`)
To wait for a task to complete and get its return value, use the `await` function:

```sema
(await p) ; => 30
```

### Concurrent Execution
If you spawn multiple tasks, they run concurrently. You can kick off several jobs and wait for all of them:

```sema
(define task1 (async (do-slow-work-1)))
(define task2 (async (do-slow-work-2)))

;; Both tasks are running. Now we wait for their results:
(define result1 (await task1))
(define result2 (await task2))
```

---

## 2. Sleeping and Yielding

Within an async task, you can pause execution to let other tasks run, or delay execution for a specific duration.

### Sleeping (`async/sleep`)
Use `async/sleep` to yield control to the scheduler for at least a certain number of milliseconds:

```sema
(async
  (println "Starting...")
  (async/sleep 1000) ; pause for 1 second
  (println "Done!"))
```

::: tip Deterministic — and real wall-clock everywhere
The scheduler uses a **virtual clock**, so sleeps order tasks deterministically — a shorter sleep always wakes before a longer one, the same on every run. The clock advances in real time: on native (a 1-second sleep really waits) and in the **browser playground**, where eval runs on a Web Worker that blocks on `Atomics.wait` so the sleep really pauses while the page stays responsive. Browsers without cross-origin isolation fall back to advancing instantly (ordering preserved). Sleep durations are capped at 1 day.
:::

---

## 3. Channels

Channels are bounded FIFO (First-In, First-Out) buffers used to communicate and synchronize data between concurrent tasks.

### Creating a Channel (`channel/new`)
Create a channel with a specific buffer capacity. The default capacity is 1:

```sema
(define ch (channel/new 3)) ; holds up to 3 values
```

### Sending and Receiving (`send` / `recv`)
- **`channel/send`** sends a value to the channel. If the channel is full, the sending task yields until space becomes available.
- **`channel/recv`** receives a value from the channel. If the channel is empty, the receiving task yields until a value is sent.

```sema
(define ch (channel/new 1))

;; A worker task sends a message:
(async (channel/send ch "message from worker"))

;; `channel/recv` only blocks (yields) inside an async task, so receive
;; from within one and await the result:
(await
  (async
    (let ((msg (channel/recv ch)))
      (println msg) ; => "message from worker"
      msg)))
```

> [!NOTE]
> Channel operations only block by yielding to the scheduler, which runs
> async tasks. Calling `channel/recv` on an empty channel (or `channel/send`
> on a full one) from the **top level** — outside any `async` task — raises an
> error instead of waiting, because there is no task to suspend.

### Closing Channels (`channel/close`)
When you are finished sending data, close the channel. Any subsequent sends will raise an error. Receivers waiting on a closed, empty channel will receive `nil`:

```sema
(channel/close ch)
```

---

## 4. Producer / Consumer Example

Here is a complete example of a producer task sending a series of numbers to a consumer task via a channel:

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
    (await consumer))) ; => 60
```

---

## 5. Async inside Higher-Order Functions

Sema's standard library functions like `map`, `filter`, and `for-each` support async callbacks. However, if you pass a *yielding* native (like `channel/recv`) directly and it actually needs to suspend, the runtime cannot yield through it. Wrap it in a lambda so the yield can suspend cleanly:

```sema
;; ❌ Inside an async task, if `channel/recv` must wait for a value it raises:
;;    "yielding native passed directly to a higher-order function"
(async (map channel/recv (list ch1 ch2)))

;; ✓ Wrap the yielding call in a lambda:
(async (map (fn (c) (channel/recv c)) (list ch1 ch2)))
```
