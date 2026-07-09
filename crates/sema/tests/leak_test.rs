//! CORE-2 leak oracles: recursive local closures and env⇄closure `define`s
//! form `Rc` cycles that reference counting alone cannot reclaim. The cycle
//! collector (`sema_core::cycle`, ADR #66, `docs/plans/2026-07-02-core2-gc.md`)
//! severs them at safe points (`make_closure` threshold, top-level eval
//! return, `Interpreter::drop`), so these tests assert BOUNDED live-heap
//! growth across churn and teardown workloads. The controls prove the
//! measurement harness itself is sound (same workload shapes without cycles
//! stay flat under plain `Rc` drop).
//!
//! Run with rates printed: `cargo test -p sema-lang --test leak_test -- --nocapture`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use sema_eval::Interpreter;

/// Wraps the system allocator, tracking net live bytes. Coarse but exactly
/// what the leak needs: a cycle keeps its allocations live forever, so net
/// growth across a churn workload measures the leak directly.
struct CountingAlloc;

static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);

// SAFETY: delegates every operation to `System`; only adds relaxed counter
// bookkeeping, which cannot violate allocator invariants.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE_BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            LIVE_BYTES.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// The test harness runs tests on separate threads; serialize so each
/// measurement sees only its own allocations.
static MEASURE_LOCK: Mutex<()> = Mutex::new(());

fn live_bytes() -> isize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Net live-heap growth of `f`, after `warmup` has populated caches,
/// the interner, and lazily-initialized statics.
fn measure(warmup: impl FnOnce(), f: impl FnOnce()) -> isize {
    warmup();
    let before = live_bytes();
    f();
    live_bytes() - before
}

// The self-call must be non-tail: a tail-only self-recursion elides its self
// capture (issue #62) and never forms the cycle this oracle measures.
const CHURN_RECURSIVE: &str = r#"
(define (churn)
  (define (loop n) (if (<= n 0) 0 (+ 1 (loop (- n 1)))))
  (loop 3))
(define (run n)
  (if (<= n 0) nil
      (begin (churn) (run (- n 1)))))
"#;

const CHURN_FLAT: &str = r#"
(define (churn)
  (define (helper n) (- n 1))
  (helper 3))
(define (run n)
  (if (<= n 0) nil
      (begin (churn) (run (- n 1)))))
"#;

const ITERS: usize = 20_000;
/// Generous per-iteration allowance for cache/interner drift. The leak is two
/// orders of magnitude above this (~300 B/iter), so the bound is not tight.
const BYTES_PER_ITER_BOUND: isize = 16;

fn assert_bounded_churn(program: &str, label: &str) {
    let _guard = MEASURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let interp = Interpreter::new();
    interp.eval_str_compiled(program).expect("defs eval");
    let grown = measure(
        || {
            interp.eval_str_compiled("(run 2000)").expect("warmup");
        },
        || {
            interp
                .eval_str_compiled(&format!("(run {ITERS})"))
                .expect("workload");
        },
    );
    let per_iter = grown / ITERS as isize;
    println!("{label}: net heap growth {grown} B over {ITERS} iters ({per_iter} B/iter)");
    assert!(
        grown < ITERS as isize * BYTES_PER_ITER_BOUND,
        "{label}: leaked {grown} bytes over {ITERS} iterations ({per_iter} B/iter); \
         bound is {BYTES_PER_ITER_BOUND} B/iter (CORE-2)"
    );
}

/// CORE-2 oracle, VM upvalue shape: each `churn` call creates a recursive
/// local closure whose self-capture is an `Rc<UpvalueCell>` closed over the
/// closure itself — a cycle plain `Rc` drop cannot reclaim (~300 B/iter
/// before the collector). The `make_closure` threshold safe point severs the
/// cells mid-workload, so growth stays bounded even inside one long eval.
#[test]
fn recursive_local_closure_growth_is_bounded() {
    assert_bounded_churn(CHURN_RECURSIVE, "recursive-local-closure churn");
}

/// Control: identical workload shape, no self-capture, no cycle. Stays flat —
/// proves the harness measures the cycle, not general eval-churn noise.
#[test]
fn nonrecursive_local_closure_growth_is_bounded() {
    assert_bounded_churn(CHURN_FLAT, "non-recursive control churn");
}

const CHURN_CHANNELS: &str = r#"
(define (churn) (channel/new 1))
(define (run n)
  (if (<= n 0) nil
      (begin (churn) (run (- n 1)))))
"#;

/// CORE-2 oracle, data-birth shape: every cold data-cycle constructor
/// (`channel/new` here) registers a collector candidate, and a dead
/// candidate's registry `Weak` pins the allocation's `RcBox` (~100 B for a
/// channel) until a pass prunes it. No closure is born in this loop, so only
/// `register_candidate`'s own registry-growth trigger can run that pass
/// mid-eval — without it, acyclic data churn retains O(total births), not
/// O(live), until the eval returns (which a server-style loop never does).
#[test]
fn acyclic_channel_churn_growth_is_bounded() {
    assert_bounded_churn(CHURN_CHANNELS, "acyclic channel-churn");
}

const TEARDOWN_ITERS: usize = 10;

/// Net growth of TEARDOWN_ITERS create→eval→drop cycles. `programs` runs as
/// successive top-level evals on the one interpreter (later programs can rely
/// on earlier ones being *executed* — e.g. a macro body reading a `define`d
/// value at expansion time).
///
/// The warmup ends with a program-less quiescing teardown: its drop-time
/// collection reclaims anything the last warmup run's drop could not (a
/// retained env would otherwise be freed by measured run #1 and mask an
/// equal-sized retention left by the final measured run — the "one-behind"
/// cancellation). The measured window therefore starts from a fully-collected
/// heap, so even a bounded last-drop-retains leak shows as growth.
fn interpreter_teardown_growth(programs: &[&str]) -> isize {
    let _guard = MEASURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let run_once = || {
        let interp = Interpreter::new();
        for src in programs {
            interp.eval_str_compiled(src).expect("eval");
        }
    };
    measure(
        || {
            (0..3).for_each(|_| run_once());
            drop(Interpreter::new());
        },
        || (0..TEARDOWN_ITERS).for_each(|_| run_once()),
    )
}

fn assert_bounded_teardown(grown: isize, label: &str, leak_desc: &str) {
    let per_drop = grown / TEARDOWN_ITERS as isize;
    println!("{label}: net growth {grown} B over {TEARDOWN_ITERS} drops ({per_drop} B/drop)");
    assert!(
        grown < TEARDOWN_ITERS as isize * 4096,
        "each dropped Interpreter leaked ~{per_drop} bytes ({leak_desc}; CORE-2)"
    );
}

/// CORE-2 oracle, Env shape: `(define (f x) x)` makes the global env bind a
/// closure whose `Closure::globals` points back at that env
/// (`Env → binding → NativeFn → VmClosurePayload → Closure → globals → Env`),
/// which would pin the ENTIRE global environment past teardown — every
/// builtin, the bindings map, all of it. `Interpreter::drop` releases its own
/// env ref and runs an unpinned collection, severing the cycle so the env is
/// freed with the interpreter.
#[test]
fn interpreter_teardown_frees_global_env() {
    let grown = interpreter_teardown_growth(&["(define (f x) x)"]);
    assert_bounded_teardown(
        grown,
        "interpreter teardown with one define",
        "whole global env pinned by the Env⇄Closure cycle",
    );
}

/// CORE-2 oracle, consts shape: a macro expanding to a live closure VALUE is
/// compiled into chunk consts (`lower_expr`'s catch-all `CoreExpr::Const`),
/// making the const a real strong edge on the Env⇄Closure cycle. The tracer
/// reports chunk consts exactly; untraced, the consts ref would act as a
/// phantom external count keeping the injected closure — and through its home
/// globals the whole global env — black at teardown.
#[test]
fn interpreter_teardown_with_macro_injected_const_frees_env() {
    let grown = interpreter_teardown_growth(&[
        "(define (f x) (* x 3))",
        "(begin (defmacro inject () f) (define (user x) ((inject) x)) (user 2))",
    ]);
    assert_bounded_teardown(
        grown,
        "interpreter teardown with macro-injected const",
        "global env pinned through an untraced chunk-consts edge",
    );
}

/// CORE-2 oracle, module shape: `import` caches export closures in
/// `EvalContext.module_cache`, and the ctx outlives the teardown collection
/// (fields drop after the `Drop` body) — so `Interpreter::drop` must clear the
/// ctx-held Value stores first, or the cached exports keep the module env and,
/// via its parent chain, the ENTIRE global env externally referenced.
#[test]
fn interpreter_teardown_with_import_frees_module_env() {
    let dir = std::env::temp_dir().join(format!("sema-leak-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("lib.sema");
    std::fs::write(
        &path,
        "(define (private-helper x) (* x 10))\n\
         (define (public-api x) (private-helper (+ x 1)))\n",
    )
    .expect("write module");
    let program = format!(
        r#"(begin (import "{}" public-api) (public-api 3))"#,
        path.display()
    );
    let grown = interpreter_teardown_growth(&[&program]);
    assert_bounded_teardown(
        grown,
        "interpreter teardown with module import",
        "global env pinned by module-cache export closures held past the teardown collect",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// CORE-2 M1 gate, builtin-delegate shape: the `register_vm_delegates` (and
/// deftool/defagent) natives capture the env they are registered into as
/// `Weak<Env>` (invariant I2), so no `Env → binding → NativeFn → Box<dyn Fn>
/// → Env` cycle exists, and `Interpreter::drop` releases the thread-local
/// scheduler's `Rc<Env>` clone. A define-free Interpreter therefore tears
/// down cleanly under plain `Rc` drop — no collector involved.
#[test]
fn interpreter_teardown_without_defines_is_bounded() {
    let grown = interpreter_teardown_growth(&[]);
    assert_bounded_teardown(
        grown,
        "interpreter teardown (no defines)",
        "no user code ran — builtin delegates strongly capture their home env",
    );
}
