> Part of [VM-per-Task Async Concurrency](./vm-per-task-async-plan.md)

# Research Notes: Async Concurrency Approaches

Findings from three parallel research investigations conducted 2026-04-13.

## 1. Can the replay log be expanded to fix side effects?

**Verdict: No.** Expanding the replay log to cover all side-effecting operations papers over return values while leaving duplicate external effects unaddressed.

**Analysis**:
- The current replay log caches return values of yieldable operations (channel/recv, async/await, async/sleep). On replay, `replay_check()` returns the cached value instead of re-executing.
- For **external I/O** (file/write, http/post, println, serial/write, sqlite/execute, kv/set, shell!): replaying the *return value* (typically nil) does not undo the *external effect*. The file was already written on the first run before the yield. On replay, the write would need to be suppressed, but `SUPPRESS_OUTPUT` is `#[allow(dead_code)]` and never wired up.
- For **set! (mutation)**: This is a special form, not a stdlib function. It operates on the Env directly and cannot be wrapped with replay_check/replay_record without modifying evaluator special-form dispatch. Mutations from the first run persist across replays because the task reuses the same environment.
- For **non-deterministic functions** (random, time/now): These *could* benefit from replay logging (would get same value on replay). Only category where the approach works, but least consequential.
- **Scale**: 40+ side-effecting functions across io.rs, http.rs, serial.rs, sqlite.rs, kv.rs, system.rs, server.rs, stream.rs, terminal.rs, meta.rs, math.rs, list.rs. Every new stdlib function would need replay-safety evaluation.

## 2. Can replay avoid re-execution?

Four approaches investigated:

### Checkpoint/Snapshot
Capture VM state (stack, frames, upvalues, inline cache) at yield, restore on resume. **Medium-high complexity.** Upvalue identity remapping is the hard problem — `Rc<UpvalueCell>` is shared between frames and closures; naive clone breaks sharing. VM-only (Env Rc chains prevent tree-walker snapshots). Excellent resume performance.

### Partial Replay (Full Memoization)
Record every eval_step / VM instruction result. **Very high complexity, poor performance.** Log grows linearly with total computation, not yield count. Expression identity is fragile (macro expansion changes Rc pointers). Not recommended.

### CPS Transform
Compiler rewrites async bodies into state machines at compile time. **Theoretically optimal** (zero runtime overhead). Full CPS across all 13 control-flow CoreExpr variants is 2-3 months. **Restricted CPS** (yield only at statement positions in begin/let) is 2-3 weeks and covers 90%+ of real use cases.

### Delimited Continuations (Stack Slicing)
At yield, slice VM stack/frames above scheduler entry, stash, splice back on resume. **Medium complexity.** Open upvalue index rebasing is simpler than full snapshot (only adjust offsets, not remap Rc identity). Builds on existing `run_cooperative`/`VmExecResult::Yielded` infrastructure.

## 3. Approach comparison

| Approach | Complexity | Timeline | Side-effect safe | Both evaluators |
|---|---|---|---|---|
| Expanded replay | Low | 1 week | **No** | Yes |
| VM-per-Task | Low | 1 week | **Yes** | VM only |
| Checkpoint/Snapshot | Medium-high | 2-3 weeks | Yes | VM only |
| Restricted CPS | Medium | 2-3 weeks | Yes | VM only |
| Full CPS | Very high | 2-3 months | Yes | VM only |
| Stack Slicing | Medium | 2 weeks | Yes | VM only |

## 4. Decision rationale

**VM-per-Task** selected because:
1. Simplest correct solution — each VM is independent, no stack manipulation or upvalue rebasing
2. Lowest risk — VM struct already works, just instantiate more
3. Memory overhead negligible for realistic task counts (tens to hundreds)
4. Same user-facing API as PR #29
5. Can upgrade to stack slicing or restricted CPS later if memory becomes a concern — scheduler API stays the same
