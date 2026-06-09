# VM: `set!` inside stdlib-HOF callbacks is silently lost (C1)

**Status:** Open (re-verified 2026-06-09 on v1.16.0). The open-upvalue migration (ADR #55) did NOT fix this.
**Severity:** HIGH — silent wrong results, backend divergence.

## Repro

```
$ sema --tw -e '(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)'
6
$ sema      -e '(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)'
0
```

Affects any closure capturing a local that is invoked via a stdlib higher-order function (`map`, `filter`, `for-each`, `sort-by`, `retry`, …). Globals are unaffected; in-VM closure calls are unaffected.

## Root cause (current, post-open-upvalues)

The Lua-style open-upvalue runtime shipped 2026-03-11 (`UpvalueState::{Open,Closed}` in `crates/sema-vm/src/vm.rs`, commits `f691a55`/`346f46d`), which fixed in-VM mutation. But the shipped variant deviates from ADR #55 point 5: the VM calls `close_open_upvalues` **before every non-VM call** (the `call_callback` sites in vm.rs — search "before non-VM call") to keep LoadLocal/StoreLocal branch-free.

When a VM closure crosses to a stdlib HOF, the HOF runs it via `NativeFn::func` on a **fresh VM** (Decision #50, the dual-path). By that point its upvalue cells are **closed snapshots** detached from the parent's stack slots. The callback's `set!` mutates the snapshot; the parent never sees it.

## Related symptoms (same dual-path root, all verified present)

- `(type (fn (x) x))` → `:native-fn` on VM, `:lambda` on tree-walker
- VM caught-error maps lack `:stack-trace` (TW has it): keys `(:expected :got :message :type)` vs TW `(... :stack-trace ...)`
- Arithmetic type-error message text differs between backends (`(+ 1 "a")`)

## Fix directions

1. **Keep cells open across the cross-VM bridge** (original ADR #55 point 5): the fresh VM reads/writes through shared `Rc<RefCell<UpvalueCell>>` while the parent's cell is still Open. Cost: reintroduces the open-cell check the close-before-call hack avoided, or requires careful aliasing rules for parent-stack access from another VM.
2. **Route HOF callbacks in-VM**: make stdlib HOFs dispatch VM closures back into the calling VM (extend the `call_callback` mechanism to carry the VM context) so the fallback fresh-VM path is never taken for same-thread calls. Also fixes the `:native-fn` type symptom and likely the missing `:stack-trace`.

See memory note "VM closure dual-path": anything yield-aware must work for both the in-VM and fallback paths — option 2 interacts with async/yield.

## Workaround

Use `--tw` for code relying on `set!`-through-HOF, or refactor to `foldl` with explicit accumulator threading.

## References

- `docs/limitations.md` #31 (user-facing entry)
- `docs/adr.md` #55 (status + deviation note), #50 (dual-path), #57 (span/trace gap)
- `docs/done/plans/2026-03-11-open-upvalues.md` (the shipped migration)
