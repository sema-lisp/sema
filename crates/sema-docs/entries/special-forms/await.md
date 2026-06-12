---
name: "await"
module: "special-forms"
syntax: "(await promise)"
---

Wait for a promise to settle and return its resolved value. If the promise was rejected, the rejection reason is re-raised as an error. Inside an async task, `await` yields control to the scheduler, allowing other tasks to run until the promise resolves. At the top level, `await` drives the scheduler inline until the promise settles. This form is **VM-only** — using the tree-walker backend (`--tw`) raises an error.

```sema
(await (async (* 6 7)))   ; => 42
```

Awaiting multiple promises in sequence lets you compose async results. Tasks that are not currently awaited continue to make progress in the background.

```sema
(let ((p1 (async (* 3 3)))
      (p2 (async (* 4 4))))
  (+ (await p1) (await p2)))   ; => 25
```

If a task throws an error, `await` re-raises it. You can wrap `await` in `try` to handle rejections gracefully.

```sema
(try
  (await (async (throw "oops")))
  (catch e
    (println "Caught:" (:message e))))
```

**Note:** `await` lowers to a call to `async/await`. It requires the VM backend; the tree-walker does not support the cooperative scheduling mechanism.
