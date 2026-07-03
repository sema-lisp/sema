//! CORE-2 cycle collector (ADR #66, `docs/plans/2026-07-02-core2-gc.md`).
//!
//! Synchronous Bacon–Rajan trial deletion (Bacon & Rajan 2001: MarkRoots /
//! ScanRoots / CollectRoots with markGray / scan / scanBlack / collectWhite)
//! adapted to run *over* `std::rc::Rc`: no per-object headers, no color bits —
//! all collection state lives in a transient side map keyed by the `Rc`
//! allocation's data pointer. Cycles are reclaimed by *severing* the mutable
//! cell every Sema cycle must pass through (invariant I1: env bindings,
//! upvalue cells, `Thunk.forced`, promise state, channel buffers, multimethod
//! tables) and letting the ordinary `Rc` drop cascade free the memory.
//!
//! Candidate discovery is a creation-time registry of the only objects that
//! can be *born into* cycles (plan §4 option B), not a decrement buffer —
//! `Value::drop` and call dispatch stay untouched. Registry entries are
//! `Weak`, so non-cyclic garbage self-prunes at zero cost.
//!
//! Thread-local, single-threaded, std-only (wasm32-compatible) — the same
//! pattern as the interner and the eval callbacks.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use hashbrown::{hash_map, HashMap, HashSet};
use lasso::Spur;

use crate::value::{AsyncPromise, Channel, MultiMethod, Thunk};
use crate::value::{Env, NativeFn, PromiseState, Value, ValueViewRef};

/// The shared bindings allocation of an [`Env`] — the env's *node identity*
/// for the collector. `Env` is clone-by-value and multiple `Env` handles (and
/// `Rc<Env>` wrappers) share one bindings `Rc`, so reachability is tracked on
/// the bindings allocation, not on any particular handle.
pub type EnvBindings = RefCell<hashbrown::HashMap<Spur, Value>>;

// ── Node identity ─────────────────────────────────────────────────

/// Identity of a traced heap allocation: the `Rc`'s data pointer.
///
/// Never dereferenced by the collector itself except through the typed
/// handles it holds; opaque participants (sema-vm's `UpvalueCell`, test node
/// types) recover their `&T` from it inside their own `trace`/`sever` fns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodePtr(*const u8);

impl std::hash::Hash for NodePtr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // One usize write, so PtrHasher::write_usize sees the raw address.
        state.write_usize(self.0 as usize);
    }
}

/// Hasher for [`NodePtr`]-keyed collector maps. Keys are unique allocation
/// addresses, so a single Fibonacci multiply (splitmix/golden-ratio constant)
/// spreads them across hashbrown's control bytes without running a
/// general-purpose byte hasher per probe — the side map takes several probes
/// per traced node, which makes this one of the hottest operations in a
/// collection pass.
#[derive(Default, Clone, Copy)]
struct PtrHasher(u64);

impl std::hash::Hasher for PtrHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // Only pointer-sized keys are expected; fold anything else in so the
        // hasher stays correct for arbitrary composite keys.
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        self.0 ^= self.0 >> 32;
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        // The multiply pushes entropy toward the high bits; fold them back
        // down because hashbrown takes the bucket index from the LOW bits
        // (aligned pointers have zero low bits, and a bare multiply keeps
        // them zero — every key would land in 1/8th of the buckets).
        let h = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 = h ^ (h >> 32);
    }
}

type BuildPtrHasher = std::hash::BuildHasherDefault<PtrHasher>;
type PtrMap<V> = HashMap<NodePtr, V, BuildPtrHasher>;
type PtrSet = HashSet<NodePtr, BuildPtrHasher>;

impl NodePtr {
    /// The raw data pointer (for opaque trace/sever fns to recover `&T`).
    pub fn raw(self) -> *const u8 {
        self.0
    }

    /// Node identity of any `Rc` allocation (works for `Rc<dyn Any>` too).
    pub fn of_rc<T: ?Sized>(rc: &Rc<T>) -> NodePtr {
        NodePtr(Rc::as_ptr(rc).cast())
    }

    /// Node identity of a cycle-capable heap value. `None` for immediates and
    /// leaf heap types (strings, bytevectors, numeric arrays, big ints,
    /// prompts, messages, conversations, streams) — leaves cannot sit on a
    /// cycle and are never given nodes.
    pub fn of_value(v: &Value) -> Option<NodePtr> {
        value_node_ptr(v)
    }

    /// Node identity of an env: its shared bindings allocation. This is the
    /// pointer to pass in `collect`'s pin set for session root envs.
    pub fn of_env_bindings(env: &Env) -> NodePtr {
        NodePtr(Rc::as_ptr(&env.bindings).cast())
    }
}

// ── Registry ──────────────────────────────────────────────────────

/// A registered cycle-birth candidate. Registered once at creation by the
/// (cold) constructors of the only objects that can be born into cycles;
/// upvalue cells and interior `Value` nodes are *discovered* during trace
/// (via [`GcEdge`]), never registered.
pub enum GcNode {
    /// A VM closure's `NativeFn` wrapper (registered by `make_closure`).
    ClosureFn(Weak<NativeFn>),
    /// An env *wrapper* allocation, registered on first home-adoption. The
    /// wrapper (not just its bindings) is the candidate so a pass can reach
    /// the whole parent chain: a cycle that closes through an ANCESTOR env's
    /// bindings (e.g. module code `set!`-ing a root binding to a closure
    /// homed in the module) is reachable from the home wrapper via `parent`
    /// edges even when no registered bindings map holds the closure.
    EnvWrapper(Weak<Env>),
    /// An env's bindings allocation (data-path registration; home adoption
    /// registers the wrapper above, which reaches the bindings anyway).
    EnvBindings(Weak<EnvBindings>),
    /// `delay` thunk (data-only cycles via `forced`).
    Thunk(Weak<Thunk>),
    /// Async promise (data-only cycles via `Resolved`).
    Promise(Weak<AsyncPromise>),
    /// Channel (data-only cycles via the buffer).
    Channel(Weak<Channel>),
    /// Multimethod (data-only cycles via the method table).
    MultiMethod(Weak<MultiMethod>),
}

impl GcNode {
    /// Current strong count of the registered allocation (0 = dead entry).
    fn strong_count(&self) -> usize {
        match self {
            GcNode::ClosureFn(w) => w.strong_count(),
            GcNode::EnvWrapper(w) => w.strong_count(),
            GcNode::EnvBindings(w) => w.strong_count(),
            GcNode::Thunk(w) => w.strong_count(),
            GcNode::Promise(w) => w.strong_count(),
            GcNode::Channel(w) => w.strong_count(),
            GcNode::MultiMethod(w) => w.strong_count(),
        }
    }

    /// Upgrade a live entry into (node identity, strong snapshot handle).
    /// The handle's own +1 on the strong count is subtracted back out by the
    /// collector's snapshot-adjust set.
    fn upgrade_handle(&self) -> Option<(NodePtr, NodeHandle)> {
        match self {
            GcNode::ClosureFn(w) => w.upgrade().map(|rc| {
                let ptr = NodePtr::of_rc(&rc);
                (ptr, NodeHandle::Value(Value::native_fn_from_rc(rc)))
            }),
            GcNode::EnvWrapper(w) => w
                .upgrade()
                .map(|rc| (NodePtr::of_rc(&rc), NodeHandle::EnvWrapper(rc))),
            GcNode::EnvBindings(w) => w
                .upgrade()
                .map(|rc| (NodePtr::of_rc(&rc), NodeHandle::Bindings(rc))),
            GcNode::Thunk(w) => w.upgrade().map(|rc| {
                let ptr = NodePtr::of_rc(&rc);
                (ptr, NodeHandle::Value(Value::thunk_from_rc(rc)))
            }),
            GcNode::Promise(w) => w.upgrade().map(|rc| {
                let ptr = NodePtr::of_rc(&rc);
                (ptr, NodeHandle::Value(Value::async_promise_from_rc(rc)))
            }),
            GcNode::Channel(w) => w.upgrade().map(|rc| {
                let ptr = NodePtr::of_rc(&rc);
                (ptr, NodeHandle::Value(Value::channel_from_rc(rc)))
            }),
            GcNode::MultiMethod(w) => w.upgrade().map(|rc| {
                let ptr = NodePtr::of_rc(&rc);
                (ptr, NodeHandle::Value(Value::multimethod_from_rc(rc)))
            }),
        }
    }
}

// ── Edges ─────────────────────────────────────────────────────────

/// Enumerates the children of an opaque node. `NodePtr` is the pointer the
/// node was reported with; the collector guarantees the allocation is alive
/// for the duration of the collection. Returns `false` if a `RefCell` it
/// needed was unavailable (aborts the collection cleanly).
pub type OpaqueTraceFn = fn(NodePtr, &mut dyn FnMut(GcEdge)) -> bool;

/// Severs an opaque node's mutable cell, returning the extracted contents so
/// the collector can defer the drop until all severing has completed (the
/// `Rc` cascade must run on a fully-severed heap).
pub type OpaqueSeverFn = fn(NodePtr) -> Option<Value>;

/// One outgoing strong reference, reported once per strong `Rc` held — trial
/// deletion is arithmetic on these, so multiplicity must be exact
/// (undercount ⇒ a leak stays; overcount ⇒ frees live data).
pub enum GcEdge<'a> {
    /// A strong reference to the heap allocation behind a `Value`
    /// (immediates and leaf heap types are ignored by the collector).
    Value(&'a Value),
    /// A strong reference to an `Rc<Env>` *wrapper* allocation (e.g.
    /// `Env.parent`, `Closure.globals`). The wrapper is its own node whose
    /// children are its bindings allocation and its parent wrapper.
    Env(&'a Rc<Env>),
    /// A strong reference to an env's shared bindings allocation, as held
    /// directly by an `Env` handle embedded by value (e.g. `Lambda.env`).
    EnvBindings(&'a Rc<EnvBindings>),
    /// A sema-vm-owned node (`UpvalueCell`) sema-core cannot type: identity +
    /// current strong count + how to enumerate its children and sever it.
    Opaque {
        ptr: NodePtr,
        strong_count: usize,
        trace: OpaqueTraceFn,
        sever: OpaqueSeverFn,
    },
}

/// Registered by sema-vm at startup (same pattern as `set_eval_callback`,
/// keeping sema-core independent of sema-vm). Reports **all** heap edges
/// owned by the whole `NativeFn` — its payload `Rc`s *including the payload
/// allocation itself* (as an [`GcEdge::Opaque`], with the exact number of
/// strong refs the `NativeFn` holds to it) and everything the boxed fn
/// captures. Returns `false` to abort the collection (unborrowable cell).
pub type PayloadTracer = fn(&Rc<dyn Any>, &mut dyn FnMut(GcEdge)) -> bool;

// ── Stats ─────────────────────────────────────────────────────────

/// Which safe point requested a collection pass. Purely observational —
/// recorded on the [`GcPassEvent`] so telemetry can attribute collector work
/// to the code path that triggered it; the pass itself runs identically for
/// every trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcTrigger {
    /// Registry growth crossed the collection threshold at a candidate birth
    /// (`make_closure` or a data-cycle constructor).
    Threshold,
    /// Top-level eval return (REPL line, script form, embedded eval).
    EvalReturn,
    /// Interpreter teardown (`Interpreter::drop`).
    InterpreterDrop,
    /// Notebook cell eval return.
    NotebookCell,
    /// Notebook kernel reset mop-up.
    NotebookReset,
    /// Agent tool-loop turn boundary.
    AgentTurn,
    /// Cooperative scheduler went idle (all tasks done and reaped).
    SchedulerIdle,
    /// Explicit request: `(gc/collect)`, REPL `,gc`, or a host call.
    Explicit,
}

impl GcTrigger {
    /// Stable lowercase-kebab name, for span/metric attributes.
    pub fn as_str(self) -> &'static str {
        match self {
            GcTrigger::Threshold => "threshold",
            GcTrigger::EvalReturn => "eval-return",
            GcTrigger::InterpreterDrop => "interpreter-drop",
            GcTrigger::NotebookCell => "notebook-cell",
            GcTrigger::NotebookReset => "notebook-reset",
            GcTrigger::AgentTurn => "agent-turn",
            GcTrigger::SchedulerIdle => "scheduler-idle",
            GcTrigger::Explicit => "explicit",
        }
    }
}

/// One collector pass, as reported to the [`set_gc_observer`] observer. Fires
/// for every pass that actually ran — including aborted ones (visible via
/// `stats.aborted`) and prune-only fast passes — but never for a
/// [`maybe_collect`] that stayed below the threshold.
#[derive(Debug, Clone, Copy)]
pub struct GcPassEvent {
    /// The safe point that requested the pass.
    pub trigger: GcTrigger,
    /// The pass's result.
    pub stats: GcStats,
    /// Registry length (live + not-yet-pruned dead entries) when the pass
    /// started.
    pub registry_len_before: usize,
    /// Wall-clock duration of the pass. Zero on wasm32 (no monotonic clock).
    pub duration_ns: u64,
}

/// Register (or clear, with `None`) the per-pass observer. Thread-local, like
/// all collector state; registered by the host's telemetry wiring (sema-otel
/// via sema-llm — sema-core cannot depend on either, the same seam as the
/// eval callbacks). The observer is a plain `fn` so it cannot capture
/// `Value`/`Env` state (invariant I2 applies to it as it does to native fns);
/// it runs after the pass has fully completed — the heap is never touched
/// mid-callback — and must not call back into the collector. When no observer
/// is registered a pass pays one thread-local `Option` load and nothing else;
/// the no-pass path (`maybe_collect` below threshold) pays nothing.
pub fn set_gc_observer(observer: Option<fn(&GcPassEvent)>) {
    GC.with(|gc| gc.observer.set(observer));
}

/// Result of one collection pass.
#[derive(Debug, Default, Clone, Copy)]
pub struct GcStats {
    /// Live registry entries scanned as trial-deletion roots.
    pub candidates: usize,
    /// Nodes visited (side-map size).
    pub traced: usize,
    /// White (garbage) nodes identified and severed/reclaimed.
    pub collected: usize,
    /// Registry entries removed: dead `Weak`s, plus duplicate entries for a
    /// live allocation (one is kept).
    pub pruned: usize,
    /// True if the pass stopped before severing anything (a needed `RefCell`
    /// was borrowed, or a collection was already running). Nothing was
    /// mutated; a later collect can reclaim.
    pub aborted: bool,
}

impl GcStats {
    /// All-zero stats (`Default`, usable in `const` contexts).
    pub const fn new() -> Self {
        GcStats {
            candidates: 0,
            traced: 0,
            collected: 0,
            pruned: 0,
            aborted: false,
        }
    }
}

// ── Thread-local collector state ──────────────────────────────────

/// Collection-trigger floor: a pass runs no earlier than this many registry
/// entries. Bounded by the churn leak oracle — the last un-collected batch
/// (≈ floor × cycle size) is what a long eval retains at its high-water mark.
const GC_FLOOR: usize = 1024;

/// Survivor multiplier for the growth threshold (CPython's generation-0
/// heuristic flattened to one generation): after a pass leaving S live
/// entries, the next threshold-triggered pass waits for the registry to
/// exceed `max(GC_FLOOR, GC_GROWTH × S)`. Live candidates are re-traced
/// every pass (a registry, unlike a decrement buffer, cannot drop a
/// proven-live entry), so the multiplier is what keeps live-closure-heavy
/// workloads from paying O(live) tracing per O(live) births: at 4×, tracing
/// S live entries is amortized over ≥ 3S births. Peak uncollected garbage is
/// bounded by the same expression (M4 measured 4× as the knee where the
/// closure-storm live-set tax drops under the gate with no oracle regression;
/// 2× doubled the pass count for no memory benefit worth having).
const GC_GROWTH: usize = 4;

/// All collector state for one thread, behind a single `thread_local` so the
/// hot path (`register_closure_birth`, one call per VM closure creation)
/// pays one TLS access instead of one per sub-structure.
struct GcState {
    registry: RefCell<Vec<GcNode>>,
    /// Seen-set for env home-adoption registration (plan §8: in the
    /// registry, no core-type change). Keyed by wrapper-allocation identity;
    /// the `Weak` value both proves liveness and pins the allocation's
    /// address against reuse while the entry exists. Pruned alongside the
    /// registry.
    env_seen: RefCell<PtrMap<Weak<Env>>>,
    payload_tracers: RefCell<HashMap<TypeId, PayloadTracer>>,
    collecting: Cell<bool>,
    last_survivors: Cell<usize>,
    /// Registry length that triggers the next threshold collect —
    /// `max(GC_FLOOR, GC_GROWTH × last survivors)`, precomputed at the end
    /// of each pass so the birth path compares two integers.
    threshold: Cell<usize>,
    /// Stats of the last *completed* (non-aborted) pass, for `(gc/stats)`.
    last_stats: Cell<GcStats>,
    /// Pass observer ([`set_gc_observer`]); `None` = observation disabled.
    observer: Cell<Option<fn(&GcPassEvent)>>,
    /// Reusable pass buffers (owned by at most one pass at a time — the
    /// `collecting` guard excludes reentry before the scratch is taken).
    scratch: RefCell<Scratch>,
}

impl GcState {
    fn new() -> Self {
        GcState {
            registry: RefCell::new(Vec::new()),
            env_seen: RefCell::new(PtrMap::default()),
            payload_tracers: RefCell::new(HashMap::new()),
            collecting: Cell::new(false),
            last_survivors: Cell::new(0),
            threshold: Cell::new(GC_FLOOR),
            last_stats: Cell::new(GcStats::new()),
            observer: Cell::new(None),
            scratch: RefCell::new(Scratch::default()),
        }
    }

    /// Registry-growth trigger — two integer loads.
    fn past_threshold(&self) -> bool {
        !self.collecting.get() && self.registry.borrow().len() > self.threshold.get()
    }

    /// Register `home` as an env-wrapper candidate on first adoption.
    fn adopt_home(&self, home: &Rc<Env>) -> bool {
        match self.env_seen.borrow_mut().entry(NodePtr::of_rc(home)) {
            hash_map::Entry::Occupied(entry) => {
                // `home` is alive at this address, and the entry's `Weak`
                // pins whatever allocation it was created from — same
                // address ⇒ same (still live) allocation.
                debug_assert!(entry.get().strong_count() > 0);
                false
            }
            hash_map::Entry::Vacant(entry) => {
                let weak = Rc::downgrade(home);
                entry.insert(weak.clone());
                self.registry.borrow_mut().push(GcNode::EnvWrapper(weak));
                true
            }
        }
    }
}

thread_local! {
    static GC: GcState = GcState::new();
}

/// Register a cycle-birth candidate. One O(1) push — no dedup: each of
/// these objects registers exactly once, at creation. Envs are the exception
/// (they register at *home adoption*, which recurs per closure) and must go
/// through [`register_env_candidate`] or [`register_closure_birth`] instead.
/// `Weak`: never keeps garbage alive; dead entries self-prune during
/// [`collect`], as does any duplicate entry for a live allocation.
///
/// Data births share the registry-growth trigger with closure births (plan
/// §5.2): a push that crosses the threshold runs a [`threshold_collect`]
/// right here, so a long eval that churns channels/thunks/promises/
/// multimethods without ever creating a closure still prunes its dead
/// entries — each one pins its allocation's `RcBox` through the `Weak` —
/// and reclaims data-only cycles mid-eval, instead of retaining O(total
/// births) until an outer safe point that a server-style `(loop …)` never
/// reaches. The pass runs unpinned (sema-core has no view of the session
/// env; pins are a pure optimization, exactly as at the agent-turn safe
/// point): acyclic churn — the common case — takes the prune-only fast path
/// and never traces, and a full trace amortizes over the ≥ 3×-survivors
/// births the growth policy requires between passes. These constructors are
/// cold (never in numeric hot loops), so the threshold compare is free in
/// practice.
pub fn register_candidate(node: GcNode) {
    let past_threshold = GC.with(|gc| {
        gc.registry.borrow_mut().push(node);
        gc.past_threshold()
    });
    if past_threshold {
        threshold_collect(&[], GcTrigger::Threshold);
    }
}

/// Register an env wrapper as a cycle candidate, exactly once per wrapper
/// allocation. Env candidates are discovered at *home adoption* (a closure
/// taking the env as its `globals` home), which repeats for every closure
/// the env homes — a workload like recursive-closure churn adopts one
/// long-lived env hundreds of thousands of times, so this entry point
/// deduplicates through a [`NodePtr`]-keyed seen-set. Returns whether this
/// call registered the env (`false` = already registered).
///
/// Address reuse is sound: a seen entry's `Weak` keeps the wrapper
/// allocation's memory pinned, so its address cannot be recycled by a fresh
/// env while the entry exists, and entries are pruned alongside the registry
/// during [`collect`].
pub fn register_env_candidate(env: &Rc<Env>) -> bool {
    GC.with(|gc| gc.adopt_home(env))
}

/// Cycle-birth registration for the VM's `make_closure` — the only hot
/// candidate producer. One call (one TLS access) registers the closure's
/// home env on first adoption and the closure wrapper itself, and reports
/// whether the registry has grown past the collection threshold — the
/// caller's cue to run a threshold safe-point [`collect`].
///
/// `home` is `None` when the caller already adopted this env (callers may
/// cache the last adopted wrapper and skip the seen-set probe); `closure`
/// is `None` when the caller proved the closure exempt from candidacy (it
/// captured zero upvalues — see the exemption argument at the
/// `make_closure` call site). Its home env is still adopted.
pub fn register_closure_birth(home: Option<&Rc<Env>>, closure: Option<&Rc<NativeFn>>) -> bool {
    GC.with(|gc| {
        if let Some(home) = home {
            gc.adopt_home(home);
        }
        if !gc.collecting.get() {
            let mut reg = gc.registry.borrow_mut();
            if let Some(nf) = closure {
                reg.push(GcNode::ClosureFn(Rc::downgrade(nf)));
            }
            reg.len() > gc.threshold.get()
        } else {
            // Defensive: no candidate producer can run inside a pass (a pass
            // runs no user code), but stay a strict no-trigger if one ever
            // does.
            if let Some(nf) = closure {
                gc.registry
                    .borrow_mut()
                    .push(GcNode::ClosureFn(Rc::downgrade(nf)));
            }
            false
        }
    })
}

/// Register the tracer for a `NativeFn.payload` concrete type. A `NativeFn`
/// whose payload has no registered tracer is treated as externally referenced
/// (pinned — never collected, never descended into): conservative and safe.
pub fn register_payload_tracer(type_id: TypeId, tracer: PayloadTracer) {
    GC.with(|gc| {
        gc.payload_tracers.borrow_mut().insert(type_id, tracer);
    });
}

fn registered_payload_tracer(type_id: TypeId) -> Option<PayloadTracer> {
    GC.with(|gc| gc.payload_tracers.borrow().get(&type_id).copied())
}

// ── Tracing ───────────────────────────────────────────────────────

/// Enumerate the outgoing heap edges of `v`'s own allocation, with exact
/// multiplicity (one `sink` call per strong `Rc` held), per the trace model
/// in the plan §3. Interior containers report their elements; the six
/// severable cells report their current contents; leaves report nothing.
/// Returns `false` if a `RefCell` was unavailable (collection must abort).
pub fn trace_value(v: &Value, sink: &mut dyn FnMut(GcEdge)) -> bool {
    match v.view_ref() {
        ValueViewRef::List(items) | ValueViewRef::Vector(items) => {
            for item in items {
                sink(GcEdge::Value(item));
            }
            true
        }
        ValueViewRef::Map(map) => {
            for (k, val) in map {
                sink(GcEdge::Value(k));
                sink(GcEdge::Value(val));
            }
            true
        }
        ValueViewRef::HashMap(map) => {
            for (k, val) in map {
                sink(GcEdge::Value(k));
                sink(GcEdge::Value(val));
            }
            true
        }
        ValueViewRef::Record(r) => {
            for field in &r.fields {
                sink(GcEdge::Value(field));
            }
            true
        }
        ValueViewRef::ToolDef(t) => {
            sink(GcEdge::Value(&t.parameters));
            sink(GcEdge::Value(&t.handler));
            true
        }
        ValueViewRef::Agent(a) => {
            for tool in &a.tools {
                sink(GcEdge::Value(tool));
            }
            true
        }
        ValueViewRef::Thunk(t) => {
            sink(GcEdge::Value(&t.body));
            match t.forced.try_borrow() {
                Ok(forced) => {
                    if let Some(fv) = forced.as_ref() {
                        sink(GcEdge::Value(fv));
                    }
                    true
                }
                Err(_) => false,
            }
        }
        ValueViewRef::AsyncPromise(p) => match p.state.try_borrow() {
            Ok(state) => {
                if let PromiseState::Resolved(rv) = &*state {
                    sink(GcEdge::Value(rv));
                }
                true
            }
            Err(_) => false,
        },
        ValueViewRef::Channel(c) => match c.buffer.try_borrow() {
            Ok(buffer) => {
                for item in buffer.iter() {
                    sink(GcEdge::Value(item));
                }
                true
            }
            Err(_) => false,
        },
        ValueViewRef::MultiMethod(m) => {
            sink(GcEdge::Value(&m.dispatch_fn));
            match m.methods.try_borrow() {
                Ok(methods) => {
                    for (k, val) in methods.iter() {
                        sink(GcEdge::Value(k));
                        sink(GcEdge::Value(val));
                    }
                }
                Err(_) => return false,
            }
            match m.default.try_borrow() {
                Ok(default) => {
                    if let Some(dv) = default.as_ref() {
                        sink(GcEdge::Value(dv));
                    }
                    true
                }
                Err(_) => false,
            }
        }
        ValueViewRef::Macro(m) => {
            // Macro bodies are literal templates; traced for safety.
            for expr in &m.body {
                sink(GcEdge::Value(expr));
            }
            true
        }
        ValueViewRef::Lambda(l) => {
            for expr in &l.body {
                sink(GcEdge::Value(expr));
            }
            // The lambda embeds its `Env` by value, so it holds one strong
            // ref to the bindings allocation and one to the parent wrapper.
            sink(GcEdge::EnvBindings(&l.env.bindings));
            if let Some(parent) = &l.env.parent {
                sink(GcEdge::Env(parent));
            }
            true
        }
        ValueViewRef::NativeFn(nf) => match &nf.payload {
            // Invariant I2: a payload-less NativeFn's box captures nothing
            // that can hold a Value or Env — it is a true leaf.
            None => true,
            Some(p) => match registered_payload_tracer(Any::type_id(&**p)) {
                Some(tracer) => tracer(p, sink),
                // Unknown payload type: report nothing here; the collector
                // pins the node (treated as externally referenced).
                None => true,
            },
        },
        // Leaves (strings, bytevectors, numeric arrays, big ints, prompts,
        // messages, conversations, streams) and immediates: no edges.
        _ => true,
    }
}

// ── Collection ────────────────────────────────────────────────────

/// Run a full synchronous collection. `pins` = node pointers whose interiors
/// are not descended into (session root envs — a pure optimization: pinned
/// nodes are externally referenced by definition and marked black
/// immediately). Caller guarantees the safe-point invariant (no outstanding
/// env/cell borrows); if a borrow is found anyway, the pass aborts cleanly
/// having mutated nothing. `trigger` is observational only (see [`GcTrigger`]).
pub fn collect(pins: &[NodePtr], trigger: GcTrigger) -> GcStats {
    collect_impl(pins, false, trigger)
}

/// Threshold safe-point collect ([`maybe_collect`] and the `make_closure`
/// birth trigger): prunes dead registry entries first, and skips the trace
/// when pruning alone brought the registry down to half the growth
/// threshold — the signature of acyclic churn, where closures die by plain
/// `Rc` drop and only their dead `Weak` entries accumulate. A skipped pass
/// does NOT update survivors or the threshold (its candidates are unproven:
/// they may include garbage cycles, and feeding them into the threshold
/// would defer cycle detection geometrically), so cyclic garbage still
/// forces a real trace as soon as it exceeds half the threshold — the same
/// memory envelope the growth policy already allows. Explicit collects
/// ([`collect`]: `(gc/collect)`, interpreter teardown) always trace.
pub fn threshold_collect(pins: &[NodePtr], trigger: GcTrigger) -> GcStats {
    collect_impl(pins, true, trigger)
}

/// Observation wrapper around [`run_pass`]: when an observer is registered,
/// time the pass and report a [`GcPassEvent`] for it (completed, prune-only,
/// or aborted alike). Unobserved passes skip straight through — one
/// thread-local `Option` load of overhead.
fn collect_impl(pins: &[NodePtr], threshold_pass: bool, trigger: GcTrigger) -> GcStats {
    let Some(observer) = GC.with(|gc| gc.observer.get()) else {
        return run_pass(pins, threshold_pass);
    };
    let registry_len_before = registry_len();
    // `Instant::now` is unavailable on wasm32-unknown-unknown; an observer
    // registered there (none is today — sema-otel is a no-op on wasm) sees
    // duration 0 rather than a panic.
    #[cfg(not(target_arch = "wasm32"))]
    let start = Some(std::time::Instant::now());
    #[cfg(target_arch = "wasm32")]
    let start: Option<std::time::Instant> = None;
    let stats = run_pass(pins, threshold_pass);
    let duration_ns = start.map_or(0, |t| t.elapsed().as_nanos() as u64);
    observer(&GcPassEvent {
        trigger,
        stats,
        registry_len_before,
        duration_ns,
    });
    stats
}

fn run_pass(pins: &[NodePtr], threshold_pass: bool) -> GcStats {
    if GC.with(|gc| gc.collecting.get()) {
        // Reentrancy guard: severing cascades `Value::drop`s, which must not
        // re-enter the collector.
        return GcStats {
            aborted: true,
            ..GcStats::default()
        };
    }
    let _guard = CollectingGuard::engage();

    // Take the reusable pass buffers (put back, reset, on every exit path).
    // `collecting` excludes reentry, so the scratch is never taken twice.
    let mut st = Collector {
        s: GC.with(|gc| std::mem::take(&mut *gc.scratch.borrow_mut())),
        aborted: false,
    };
    st.s.pins.extend(pins.iter().copied());

    // 1. Snapshot + prune: upgrade live registry entries into strong handles
    //    and seed them straight into the side map (residual = strong − 1,
    //    the handle's own +1 pre-subtracted), drop dead ones. Duplicate
    //    registrations of one live allocation are pruned here — the extra
    //    handle is dropped before any other count is read, and one entry
    //    suffices to keep the object a candidate (so duplicates don't
    //    inflate the registry or the survivor-derived threshold).
    let mut pruned = 0usize;
    GC.with(|gc| {
        gc.registry
            .borrow_mut()
            .retain(|node| match node.upgrade_handle() {
                Some((ptr, handle)) => {
                    if st.s.nodes.contains_key(&ptr) {
                        pruned += 1;
                        false
                    } else {
                        st.seed_candidate(ptr, &handle);
                        st.s.snapshot.push((ptr, handle));
                        true
                    }
                }
                None => {
                    pruned += 1;
                    false
                }
            });
    });
    let candidates = st.s.snapshot.len();

    // Prune-only fast pass: see [`threshold_collect`]. `candidates` counts
    // live-at-snapshot entries only, so the comparison is against what the
    // prune could not remove. Nothing has been traced yet (seeded nodes are
    // queued, not descended), so bailing here costs only the seeding.
    if threshold_pass && candidates <= GC.with(|gc| gc.threshold.get()) / 2 {
        let stats = GcStats {
            candidates,
            traced: 0,
            collected: 0,
            pruned,
            aborted: false,
        };
        let mut scratch = st.s;
        scratch.reset();
        GC.with(|gc| {
            *gc.scratch.borrow_mut() = scratch;
            gc.last_stats.set(stats);
        });
        return stats;
    }

    // 2. MarkGray: trial-delete from every seeded candidate over a shared
    //    side map, so overlapping subgraphs are traced once.
    st.drain_pending();
    if st.aborted {
        // Nothing has been mutated: drop the side map and snapshot untouched.
        let stats = GcStats {
            candidates,
            traced: st.s.nodes.len(),
            collected: 0,
            pruned,
            aborted: true,
        };
        let mut scratch = st.s;
        scratch.reset();
        GC.with(|gc| *gc.scratch.borrow_mut() = scratch);
        return stats;
    }

    // 3. Scan: residual count > 0 ⇒ externally referenced ⇒ scan_black
    //    (restore counts, blacken transitively); the rest tentatively white.
    for i in 0..st.s.snapshot.len() {
        let root = st.s.snapshot[i].0;
        st.scan_node(root);
    }

    // 4. CollectWhite: identify the full white set first, then sever. All
    //    extracted cell contents are deferred into `Scratch::severed` so the
    //    Rc drop cascade runs on a fully-severed heap.
    let collected = st.collect_white();
    let traced = st.s.nodes.len();

    // Release pass state (side-map handles, then severed values, then the
    // snapshot — see `Scratch::reset`); the Rc cascade happens here, after
    // all severing completed.
    let mut scratch = st.s;
    scratch.reset();
    GC.with(|gc| *gc.scratch.borrow_mut() = scratch);

    // Entries reclaimed by the cascade above are dead now; prune them so the
    // survivor count (and the growth threshold derived from it) is exact.
    // A pass that severed nothing ran no cascade — liveness is unchanged
    // since the snapshot prune, so the sweep (a strong-count read per entry)
    // is skipped and the snapshot's live count is the survivor count.
    GC.with(|gc| {
        let survivors = if collected == 0 {
            gc.registry.borrow().len()
        } else {
            let mut reg = gc.registry.borrow_mut();
            reg.retain(|node| {
                let live = node.strong_count() > 0;
                if !live {
                    pruned += 1;
                }
                live
            });
            let survivors = reg.len();
            drop(reg);
            // The env seen-set prunes in lockstep: dropping a dead entry's
            // `Weak` unpins the allocation, so a later env reusing the
            // address is correctly treated as unseen. (An aborted pass skips
            // this; the next completed pass catches up — stale dead entries
            // can never match a live env.)
            gc.env_seen
                .borrow_mut()
                .retain(|_, weak| weak.strong_count() > 0);
            survivors
        };
        gc.last_survivors.set(survivors);
        gc.threshold
            .set(std::cmp::max(GC_FLOOR, GC_GROWTH * survivors));
    });

    let stats = GcStats {
        candidates,
        traced,
        collected,
        pruned,
        aborted: false,
    };
    GC.with(|gc| gc.last_stats.set(stats));
    stats
}

/// Stats of the last completed collection pass (all-zero before the first
/// one). Aborted passes mutate nothing and are not recorded.
pub fn last_stats() -> GcStats {
    GC.with(|gc| gc.last_stats.get())
}

/// Current registry length (live + not-yet-pruned dead entries) — the value
/// the growth threshold is checked against.
pub fn registry_len() -> usize {
    GC.with(|gc| gc.registry.borrow().len())
}

/// True when the registry has grown past the collection threshold and no
/// collection is already running — the cheap pre-check that lets safe
/// points skip building a pin set when no pass would run. (`make_closure`
/// gets this for free from [`register_closure_birth`]'s return value.)
pub fn should_collect() -> bool {
    GC.with(|gc| gc.past_threshold())
}

/// Pin set for a session root env: the wrapper allocation, its bindings, and
/// every ancestor wrapper/bindings up the parent chain. Passing this to
/// [`collect`]/[`maybe_collect`] keeps a pass from descending the live global
/// namespace (a pure optimization — pinned nodes are externally referenced by
/// definition).
pub fn env_chain_pins(env: &Rc<Env>) -> Vec<NodePtr> {
    let mut pins = vec![NodePtr::of_rc(env), NodePtr::of_env_bindings(env)];
    let mut parent = env.parent.clone();
    while let Some(p) = parent {
        pins.push(NodePtr::of_rc(&p));
        pins.push(NodePtr::of_env_bindings(&p));
        parent = p.parent.clone();
    }
    pins
}

/// Threshold-gated [`threshold_collect`] for safe points: runs when the
/// registry has grown past `max(GC_FLOOR, GC_GROWTH × survivors of the last
/// collect)` (CPython's generation-0 heuristic flattened to one generation).
pub fn maybe_collect(pins: &[NodePtr], trigger: GcTrigger) -> Option<GcStats> {
    should_collect().then(|| threshold_collect(pins, trigger))
}

/// RAII guard for the thread-local `collecting` flag: engaged for the whole
/// pass, released on every exit path (including abort).
struct CollectingGuard;

impl CollectingGuard {
    fn engage() -> Self {
        GC.with(|gc| gc.collecting.set(true));
        CollectingGuard
    }
}

impl Drop for CollectingGuard {
    fn drop(&mut self) {
        GC.with(|gc| gc.collecting.set(false));
    }
}

// ── Side map ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    /// Visited by MarkGray; membership in a garbage cycle still open.
    Gray,
    /// Proven externally referenced (or pinned); kept.
    Black,
    /// Trial deletion zeroed every strong count: garbage, to be severed.
    White,
}

/// Strong handle to a traced node, kept in the side map so every traced
/// allocation stays alive (and severable) for the duration of the pass.
/// Cloned/taken only *after* the node's strong count has been recorded, so
/// the handle itself is invisible to the trial-deletion arithmetic.
#[derive(Clone)]
enum NodeHandle {
    /// Any cycle-capable heap value (containers, thunk, channel, promise,
    /// multimethod, macro, lambda, NativeFn).
    Value(Value),
    /// An `Rc<Env>` wrapper allocation.
    EnvWrapper(Rc<Env>),
    /// An env's shared bindings allocation.
    Bindings(Rc<EnvBindings>),
    /// A foreign node: no owned handle (the graph keeps it alive), just its
    /// trace/sever behavior.
    Opaque {
        trace: OpaqueTraceFn,
        sever: OpaqueSeverFn,
    },
}

struct NodeState {
    /// Residual strong count: seeded from `Rc::strong_count` (minus the
    /// snapshot's own handle), decremented once per traced incoming edge.
    count: isize,
    color: Color,
    /// Whether MarkGray enumerated this node's children (pinned and
    /// unknown-payload nodes are never descended).
    descended: bool,
    /// `(start, len)` range into the pass's shared edge arena
    /// ([`Scratch::edges`]): the node's outgoing edges recorded during
    /// MarkGray, with multiplicity — the later phases replay these instead
    /// of re-tracing, so no `RefCell` is touched again until severing, and
    /// no per-node `Vec` is allocated (a churn pass traces thousands of
    /// nodes; one arena beats thousands of tiny allocations).
    children: (u32, u32),
    handle: NodeHandle,
}

/// Reusable collection buffers, kept in a thread-local and recycled across
/// passes (`clear()` keeps capacity): threshold-driven passes run every ~1k
/// closure births under churn, and rebuilding the side map from scratch —
/// growth rehashes included — dominated pass cost before reuse.
#[derive(Default)]
struct Scratch {
    nodes: PtrMap<NodeState>,
    /// Shared children arena; see [`NodeState::children`].
    edges: Vec<NodePtr>,
    /// MarkGray worklist: nodes inserted but not yet descended. Every phase
    /// walks the graph with explicit worklists rather than Rust recursion —
    /// graph depth is user-controlled (deeply nested lists, long env parent
    /// chains), and a per-level native stack frame overflows (uncatchable
    /// SIGABRT) at depths ordinary Sema data reaches.
    pending: Vec<NodePtr>,
    pins: PtrSet,
    /// Live registry entries upgraded into strong handles for the pass.
    snapshot: Vec<(NodePtr, NodeHandle)>,
    /// The complete white set, identified before any severing starts.
    whites: Vec<NodePtr>,
    /// Extracted cell contents, dropped only after all severing completed.
    severed: Vec<Value>,
    /// Scan worklist (disjoint from `black_work`: scan_black runs while a
    /// scan_node traversal is still in flight).
    scan_work: Vec<NodePtr>,
    black_work: Vec<NodePtr>,
}

impl Scratch {
    /// Drop pass state and return the buffers to capacity-preserving empty.
    /// Order matters: side-map handles first, then the extracted severed
    /// contents, then the snapshot — the `Rc` drop cascade must run on a
    /// fully-severed heap, exactly like the local-variable drop order the
    /// collector used before buffer reuse.
    fn reset(&mut self) {
        self.nodes.clear();
        self.severed.clear();
        self.snapshot.clear();
        self.edges.clear();
        self.pending.clear();
        self.pins.clear();
        self.whites.clear();
        self.scan_work.clear();
        self.black_work.clear();
    }
}

struct Collector {
    s: Scratch,
    aborted: bool,
}

impl Collector {
    // -- MarkGray --

    /// Seed a snapshot candidate into the side map: residual count starts at
    /// `strong − 1` (the snapshot handle's own +1 pre-subtracted), colored
    /// gray and queued for descent — trial deletion then treats it exactly
    /// like a discovered node. Candidates are starting points, not edges:
    /// no decrement beyond the handle adjustment.
    fn seed_candidate(&mut self, ptr: NodePtr, handle: &NodeHandle) {
        let strong = match handle {
            NodeHandle::Value(v) => v
                .heap_strong_count()
                .expect("registered candidates are heap allocations"),
            NodeHandle::EnvWrapper(rc) => Rc::strong_count(rc),
            NodeHandle::Bindings(rc) => Rc::strong_count(rc),
            NodeHandle::Opaque { .. } => {
                unreachable!("opaque nodes are discovered via edges, never registered")
            }
        };
        let pin = matches!(handle, NodeHandle::Value(v) if has_unknown_payload(v));
        self.insert_node(ptr, strong - 1, pin, handle.clone());
    }

    /// MarkGray driver: descend queued nodes until the worklist is empty.
    /// Each node is queued exactly once (at insert), so this is one bounded
    /// pass over the subgraph with O(1) native stack per node.
    fn drain_pending(&mut self) {
        while let Some(ptr) = self.s.pending.pop() {
            if self.aborted {
                return;
            }
            self.descend(ptr);
        }
    }

    /// Process one traced edge: ensure the target node exists (queueing it
    /// for descent) and decrement its residual count for this edge. Returns
    /// the target's pointer so the caller can record the edge for the later
    /// phases (`None` for immediates and leaves, which never become nodes).
    fn gray_edge(&mut self, edge: GcEdge<'_>) -> Option<NodePtr> {
        match edge {
            GcEdge::Value(v) => {
                let ptr = value_node_ptr(v)?;
                if !self.s.nodes.contains_key(&ptr) {
                    let strong = v
                        .heap_strong_count()
                        .expect("node values are heap allocations");
                    let pin = has_unknown_payload(v);
                    self.insert_node(ptr, strong, pin, NodeHandle::Value(v.clone()));
                }
                self.dec(ptr);
                Some(ptr)
            }
            GcEdge::Env(rc) => {
                let ptr = NodePtr::of_rc(rc);
                if !self.s.nodes.contains_key(&ptr) {
                    let strong = Rc::strong_count(rc);
                    self.insert_node(ptr, strong, false, NodeHandle::EnvWrapper(rc.clone()));
                }
                self.dec(ptr);
                Some(ptr)
            }
            GcEdge::EnvBindings(rc) => {
                let ptr = NodePtr::of_rc(rc);
                if !self.s.nodes.contains_key(&ptr) {
                    let strong = Rc::strong_count(rc);
                    self.insert_node(ptr, strong, false, NodeHandle::Bindings(rc.clone()));
                }
                self.dec(ptr);
                Some(ptr)
            }
            GcEdge::Opaque {
                ptr,
                strong_count,
                trace,
                sever,
            } => {
                if !self.s.nodes.contains_key(&ptr) {
                    self.insert_node(
                        ptr,
                        strong_count,
                        false,
                        NodeHandle::Opaque { trace, sever },
                    );
                }
                self.dec(ptr);
                Some(ptr)
            }
        }
    }

    /// First sighting of a node: record its residual count (the caller has
    /// already excluded any snapshot-handle contribution), color it, and
    /// queue it for descent. Pinned nodes are black from birth and never
    /// descended (never queued).
    fn insert_node(&mut self, ptr: NodePtr, count: usize, pinned_extra: bool, handle: NodeHandle) {
        let pinned = pinned_extra || self.s.pins.contains(&ptr);
        self.s.nodes.insert(
            ptr,
            NodeState {
                count: count as isize,
                color: if pinned { Color::Black } else { Color::Gray },
                descended: pinned,
                children: (0, 0),
                handle,
            },
        );
        if !pinned {
            self.s.pending.push(ptr);
        }
    }

    fn dec(&mut self, ptr: NodePtr) {
        self.s
            .nodes
            .get_mut(&ptr)
            .expect("dec target was just ensured")
            .count -= 1;
    }

    /// Enumerate a node's children once, decrementing each target and
    /// recording the edge range (with multiplicity) for the later phases.
    /// Newly discovered children are queued on the worklist, not descended
    /// inline — native stack use is O(1) regardless of graph depth, and each
    /// node's edges land in one contiguous arena slice (descents never nest).
    fn descend(&mut self, ptr: NodePtr) {
        let handle = match self.s.nodes.get_mut(&ptr) {
            Some(node) if !node.descended => {
                node.descended = true;
                node.handle.clone()
            }
            _ => return,
        };
        let start = self.s.edges.len();
        let ok = {
            let this = &mut *self;
            let mut sink = |edge: GcEdge<'_>| {
                if let Some(child) = this.gray_edge(edge) {
                    this.s.edges.push(child);
                }
            };
            match &handle {
                NodeHandle::Value(v) => trace_value(v, &mut sink),
                NodeHandle::EnvWrapper(env) => {
                    sink(GcEdge::EnvBindings(&env.bindings));
                    if let Some(parent) = &env.parent {
                        sink(GcEdge::Env(parent));
                    }
                    true
                }
                NodeHandle::Bindings(bindings) => match bindings.try_borrow() {
                    Ok(map) => {
                        for value in map.values() {
                            sink(GcEdge::Value(value));
                        }
                        true
                    }
                    Err(_) => false,
                },
                NodeHandle::Opaque { trace, .. } => trace(ptr, &mut sink),
            }
        };
        if !ok {
            self.aborted = true;
            return;
        }
        let len = self.s.edges.len() - start;
        self.s
            .nodes
            .get_mut(&ptr)
            .expect("descended node exists")
            .children = (start as u32, len as u32);
    }

    // -- Scan / ScanBlack (replayed on the recorded side graph; explicit
    //    worklists, same depth rationale as `pending`) --

    /// Partition the gray subgraph under `root`: residual count > 0 ⇒
    /// externally referenced ⇒ [`Self::scan_black`]; residual 0 ⇒ tentatively
    /// white, children scanned in turn. Order-independent: counts only grow
    /// in this phase, and every increment blackens its target, so a node
    /// still gray when popped carries exactly its MarkGray residual.
    fn scan_node(&mut self, root: NodePtr) {
        debug_assert!(self.s.scan_work.is_empty());
        self.s.scan_work.push(root);
        while let Some(ptr) = self.s.scan_work.pop() {
            let Some(node) = self.s.nodes.get_mut(&ptr) else {
                continue;
            };
            if node.color != Color::Gray {
                continue;
            }
            if node.count > 0 {
                self.scan_black(ptr);
            } else {
                node.color = Color::White;
                let (start, len) = node.children;
                let range = start as usize..(start + len) as usize;
                self.s.scan_work.extend_from_slice(&self.s.edges[range]);
            }
        }
    }

    /// Externally referenced: blacken the subgraph and restore the counts
    /// trial deletion took (one re-increment per recorded edge). Each node is
    /// blackened at most once and its edges replayed exactly once, so the
    /// restore arithmetic is exact. (Separate worklist from `scan_node`'s: a
    /// scan traversal is still in flight when this runs.)
    fn scan_black(&mut self, root: NodePtr) {
        self.s
            .nodes
            .get_mut(&root)
            .expect("scan_black node exists")
            .color = Color::Black;
        debug_assert!(self.s.black_work.is_empty());
        self.s.black_work.push(root);
        while let Some(ptr) = self.s.black_work.pop() {
            let (start, len) = self.s.nodes[&ptr].children;
            for i in start as usize..(start + len) as usize {
                let child = self.s.edges[i];
                let child_node = self.s.nodes.get_mut(&child).expect("recorded child exists");
                child_node.count += 1;
                if child_node.color != Color::Black {
                    child_node.color = Color::Black;
                    self.s.black_work.push(child);
                }
            }
        }
    }

    // -- CollectWhite --

    /// Identify the complete white set, then sever: version-bump every white
    /// env wrapper first (inline-cache hygiene), then clear each white node's
    /// mutable cell, deferring all extracted contents into `Scratch::severed`.
    fn collect_white(&mut self) -> usize {
        debug_assert!(self.s.whites.is_empty());
        let whites = &mut self.s.whites;
        whites.extend(
            self.s
                .nodes
                .iter()
                .filter(|(_, node)| node.color == Color::White)
                .map(|(ptr, _)| *ptr),
        );
        for i in 0..self.s.whites.len() {
            let ptr = self.s.whites[i];
            if let NodeHandle::EnvWrapper(env) = &self.s.nodes[&ptr].handle {
                sever_white_env_wrapper(env);
            }
        }
        for i in 0..self.s.whites.len() {
            let ptr = self.s.whites[i];
            sever_node(ptr, &self.s.nodes[&ptr].handle, &mut self.s.severed);
        }
        self.s.whites.len()
    }
}

/// True for a `NativeFn` carrying a payload whose type has no registered
/// tracer: its edges are invisible, so the node is pinned (kept) instead.
fn has_unknown_payload(v: &Value) -> bool {
    match v.view_ref() {
        ValueViewRef::NativeFn(nf) => match &nf.payload {
            Some(p) => registered_payload_tracer(Any::type_id(&**p)).is_none(),
            None => false,
        },
        _ => false,
    }
}

/// Clear a white node's mutable cell per the plan §3 "severed how" column,
/// extracting the contents into `severed` (dropped by the caller after all
/// severing completes). White nodes are unreachable from any live
/// borrow-holder — a held borrow implies a live stack reference implies an
/// unaccounted strong count implies black — so the `try_borrow_mut`s here
/// cannot fail; that impossibility is debug-asserted, and in release a
/// failure degrades to keeping the node (leak-safe).
fn sever_node(ptr: NodePtr, handle: &NodeHandle, severed: &mut Vec<Value>) {
    match handle {
        NodeHandle::Bindings(bindings) => match bindings.try_borrow_mut() {
            Ok(mut map) => severed.extend(map.drain().map(|(_, value)| value)),
            Err(_) => debug_assert!(false, "white env bindings borrowed during severing"),
        },
        // Version bump already done in the first pass; the parent edge is
        // immutable and dies with the wrapper in the cascade.
        NodeHandle::EnvWrapper(_) => {}
        NodeHandle::Opaque { sever, .. } => severed.extend(sever(ptr)),
        NodeHandle::Value(v) => match v.view_ref() {
            ValueViewRef::Thunk(t) => match t.forced.try_borrow_mut() {
                Ok(mut forced) => severed.extend(forced.take()),
                Err(_) => debug_assert!(false, "white thunk borrowed during severing"),
            },
            ValueViewRef::AsyncPromise(p) => match p.state.try_borrow_mut() {
                Ok(mut state) => {
                    // An unreachable promise has no observers, so the state
                    // change is unobservable.
                    let old = std::mem::replace(&mut *state, PromiseState::Resolved(Value::NIL));
                    if let PromiseState::Resolved(value) = old {
                        severed.push(value);
                    }
                }
                Err(_) => debug_assert!(false, "white promise borrowed during severing"),
            },
            ValueViewRef::Channel(c) => match c.buffer.try_borrow_mut() {
                Ok(mut buffer) => severed.extend(buffer.drain(..)),
                Err(_) => debug_assert!(false, "white channel borrowed during severing"),
            },
            ValueViewRef::MultiMethod(m) => {
                match m.methods.try_borrow_mut() {
                    Ok(mut methods) => {
                        for (k, value) in std::mem::take(&mut *methods) {
                            severed.push(k);
                            severed.push(value);
                        }
                    }
                    Err(_) => debug_assert!(false, "white multimethod borrowed during severing"),
                }
                match m.default.try_borrow_mut() {
                    Ok(mut default) => severed.extend(default.take()),
                    Err(_) => debug_assert!(false, "white multimethod borrowed during severing"),
                }
            }
            // Containers, NativeFn, Lambda, Macro: no severable cell of
            // their own — reclaimed by the cascade once the cells above are
            // cleared (invariant I1: every cycle passes through one).
            _ => {}
        },
    }
}

// ── Severing helpers ──────────────────────────────────────────────

/// Sever a white env *wrapper*: bump the version cell so any surviving VM
/// inline-cache entry keyed on (env, version) cannot serve a stale read once
/// the shared bindings map is cleared. The wrapper owns no severable cell of
/// its own (`parent` is immutable); version hygiene is its entire severing
/// step, and it runs before any white bindings map is drained.
fn sever_white_env_wrapper(env: &Env) {
    env.bump_version();
}

// ── Internal node classification ──────────────────────────────────

/// Node pointer for cycle-capable heap values; `None` for immediates and
/// leaf heap types (which can never sit on a cycle).
fn value_node_ptr(v: &Value) -> Option<NodePtr> {
    let ptr = v.heap_ptr()?;
    match v.view_ref() {
        ValueViewRef::List(_)
        | ValueViewRef::Vector(_)
        | ValueViewRef::Map(_)
        | ValueViewRef::HashMap(_)
        | ValueViewRef::Record(_)
        | ValueViewRef::ToolDef(_)
        | ValueViewRef::Agent(_)
        | ValueViewRef::Thunk(_)
        | ValueViewRef::MultiMethod(_)
        | ValueViewRef::Channel(_)
        | ValueViewRef::AsyncPromise(_)
        | ValueViewRef::Macro(_)
        | ValueViewRef::Lambda(_)
        | ValueViewRef::NativeFn(_) => Some(NodePtr(ptr)),
        _ => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::intern;
    use std::collections::{BTreeMap, VecDeque};

    // -- Test payload types (stand-ins for sema-vm's VmClosurePayload /
    //    UpvalueCell, exercising the payload-tracer and Opaque paths without
    //    a sema-vm dependency) --

    /// Payload holding a strong `Rc<Env>` wrapper (shape E's closure→env edge).
    struct EnvPayload {
        env: Rc<Env>,
    }

    fn env_payload_tracer(p: &Rc<dyn Any>, sink: &mut dyn FnMut(GcEdge)) -> bool {
        // The whole NativeFn holds exactly one strong ref to the payload
        // allocation (the `payload` field; the test fn's box captures nothing).
        sink(GcEdge::Opaque {
            ptr: NodePtr::of_rc(p),
            strong_count: Rc::strong_count(p),
            trace: env_payload_trace,
            sever: no_sever,
        });
        true
    }

    fn env_payload_trace(ptr: NodePtr, sink: &mut dyn FnMut(GcEdge)) -> bool {
        // SAFETY: `ptr` is the data pointer of a live `Rc<EnvPayload>` — the
        // collector keeps every traced allocation alive for the duration of
        // the collection (snapshot + side-map handles + deferred drops).
        let payload = unsafe { &*(ptr.raw() as *const EnvPayload) };
        sink(GcEdge::Env(&payload.env));
        true
    }

    fn no_sever(_: NodePtr) -> Option<Value> {
        None
    }

    /// A mutable cell node (UpvalueCell stand-in), participating via Opaque.
    struct TestCell {
        slot: RefCell<Value>,
    }

    /// Payload holding a strong `Rc<TestCell>` (shape U's closure→cell edge).
    struct CellPayload {
        cell: Rc<TestCell>,
    }

    fn cell_payload_tracer(p: &Rc<dyn Any>, sink: &mut dyn FnMut(GcEdge)) -> bool {
        sink(GcEdge::Opaque {
            ptr: NodePtr::of_rc(p),
            strong_count: Rc::strong_count(p),
            trace: cell_payload_trace,
            sever: no_sever,
        });
        true
    }

    fn cell_payload_trace(ptr: NodePtr, sink: &mut dyn FnMut(GcEdge)) -> bool {
        // SAFETY: as in env_payload_trace — live Rc<CellPayload> data pointer.
        let payload = unsafe { &*(ptr.raw() as *const CellPayload) };
        sink(GcEdge::Opaque {
            ptr: NodePtr::of_rc(&payload.cell),
            strong_count: Rc::strong_count(&payload.cell),
            trace: test_cell_trace,
            sever: test_cell_sever,
        });
        true
    }

    fn test_cell_trace(ptr: NodePtr, sink: &mut dyn FnMut(GcEdge)) -> bool {
        // SAFETY: as above — live Rc<TestCell> data pointer.
        let cell = unsafe { &*(ptr.raw() as *const TestCell) };
        match cell.slot.try_borrow() {
            Ok(slot) => {
                sink(GcEdge::Value(&slot));
                true
            }
            Err(_) => false,
        }
    }

    fn test_cell_sever(ptr: NodePtr) -> Option<Value> {
        // SAFETY: as above — live Rc<TestCell> data pointer.
        let cell = unsafe { &*(ptr.raw() as *const TestCell) };
        match cell.slot.try_borrow_mut() {
            Ok(mut slot) => Some(std::mem::replace(&mut *slot, Value::NIL)),
            Err(_) => {
                debug_assert!(false, "white cell borrowed during severing");
                None
            }
        }
    }

    /// Builds the shape-E graph: env bindings → NativeFn → payload → env
    /// wrapper → same bindings. Returns (env, wrapper, payload rc, nf rc).
    #[allow(clippy::type_complexity)]
    fn build_env_nativefn_cycle() -> (Env, Rc<Env>, Rc<EnvPayload>, Rc<NativeFn>) {
        register_payload_tracer(TypeId::of::<EnvPayload>(), env_payload_tracer);
        let env = Env::new();
        let wrapper = Rc::new(env.clone());
        let payload = Rc::new(EnvPayload {
            env: wrapper.clone(),
        });
        let nf = Rc::new(NativeFn::with_payload(
            "cyclic",
            payload.clone() as Rc<dyn Any>,
            |_, _| Ok(Value::NIL),
        ));
        env.set(intern("self"), Value::native_fn_from_rc(nf.clone()));
        (env, wrapper, payload, nf)
    }

    // 1a. env⇄nativefn garbage cycle is collected.
    #[test]
    fn env_nativefn_cycle_collected() {
        let (env, wrapper, payload, nf) = build_env_nativefn_cycle();
        let weak_nf = Rc::downgrade(&nf);
        let weak_bindings = Rc::downgrade(&env.bindings);
        register_candidate(GcNode::ClosureFn(Rc::downgrade(&nf)));
        register_candidate(GcNode::EnvBindings(Rc::downgrade(&env.bindings)));
        drop((env, wrapper, payload, nf));
        assert!(
            weak_nf.upgrade().is_some(),
            "cycle keeps the graph alive pre-collect"
        );

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.candidates, 2, "closure + env bindings registered");
        assert_eq!(stats.traced, 4, "nf + payload + wrapper + bindings");
        assert_eq!(stats.collected, 4);
        assert!(weak_nf.upgrade().is_none(), "NativeFn reclaimed");
        assert!(weak_bindings.upgrade().is_none(), "env bindings reclaimed");
    }

    // 1b. same shape with an external strong ref: kept and still usable.
    #[test]
    fn env_nativefn_cycle_with_external_ref_kept() {
        let (env, wrapper, payload, nf) = build_env_nativefn_cycle();
        let weak_nf = Rc::downgrade(&nf);
        register_candidate(GcNode::ClosureFn(Rc::downgrade(&nf)));
        let keeper = wrapper.clone();
        drop((env, wrapper, payload, nf));

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 0, "externally referenced: nothing severed");
        assert!(weak_nf.upgrade().is_some());
        let looked_up = keeper.get(intern("self"));
        assert!(looked_up.is_some(), "binding survives and resolves");
        assert!(looked_up.unwrap().is_native_fn());
    }

    // 2. cycle through an immutable list: cell → list → nativefn → cell.
    #[test]
    fn cycle_through_immutable_list_collected() {
        register_payload_tracer(TypeId::of::<CellPayload>(), cell_payload_tracer);
        let cell = Rc::new(TestCell {
            slot: RefCell::new(Value::NIL),
        });
        let payload = Rc::new(CellPayload { cell: cell.clone() });
        let nf = Rc::new(NativeFn::with_payload(
            "cell-closure",
            payload.clone() as Rc<dyn Any>,
            |_, _| Ok(Value::NIL),
        ));
        *cell.slot.borrow_mut() = Value::list(vec![Value::native_fn_from_rc(nf.clone())]);
        let weak_nf = Rc::downgrade(&nf);
        let weak_cell = Rc::downgrade(&cell);
        register_candidate(GcNode::ClosureFn(Rc::downgrade(&nf)));
        drop((cell, payload, nf));

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 4, "nf + payload + cell + list");
        assert!(weak_nf.upgrade().is_none());
        assert_eq!(weak_cell.strong_count(), 0);
    }

    // 3. Thunk.forced self-cycle collected; live unforced thunk kept intact.
    #[test]
    fn forced_thunk_self_cycle_collected() {
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        *t.forced.borrow_mut() = Some(Value::thunk_from_rc(t.clone()));
        let weak = Rc::downgrade(&t);
        register_candidate(GcNode::Thunk(Rc::downgrade(&t)));
        drop(t);

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 1);
        assert_eq!(weak.strong_count(), 0, "thunk reclaimed");
    }

    #[test]
    fn live_unforced_thunk_kept_intact() {
        let body = Value::list(vec![Value::int(1)]);
        let t = Rc::new(Thunk {
            body: body.clone(),
            forced: RefCell::new(None),
        });
        register_candidate(GcNode::Thunk(Rc::downgrade(&t)));

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 0);
        assert!(t.forced.borrow().is_none(), "forced cell untouched");
        assert_eq!(t.body, body, "body untouched");
    }

    // 4. channel containing itself (via a Value wrapping it).
    #[test]
    fn channel_containing_itself_collected() {
        let ch = Rc::new(Channel {
            buffer: RefCell::new(VecDeque::new()),
            capacity: 4,
            closed: Cell::new(false),
        });
        ch.buffer
            .borrow_mut()
            .push_back(Value::channel_from_rc(ch.clone()));
        let weak = Rc::downgrade(&ch);
        register_candidate(GcNode::Channel(Rc::downgrade(&ch)));
        drop(ch);

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 1);
        assert_eq!(weak.strong_count(), 0, "channel reclaimed");
    }

    // 5. multimethod whose method value reaches back to it.
    #[test]
    fn multimethod_method_cycle_collected() {
        let mm = Rc::new(MultiMethod {
            name: intern("mm"),
            dispatch_fn: Value::NIL,
            methods: RefCell::new(BTreeMap::new()),
            default: RefCell::new(None),
        });
        mm.methods
            .borrow_mut()
            .insert(Value::keyword("k"), Value::multimethod_from_rc(mm.clone()));
        let weak = Rc::downgrade(&mm);
        register_candidate(GcNode::MultiMethod(Rc::downgrade(&mm)));
        drop(mm);

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 1);
        assert_eq!(weak.strong_count(), 0, "multimethod reclaimed");
    }

    // 5b. promise resolved to itself (data-only cycle via Resolved).
    #[test]
    fn promise_resolved_to_itself_collected() {
        let p = Rc::new(AsyncPromise {
            state: RefCell::new(PromiseState::Pending),
            task_id: Cell::new(0),
        });
        *p.state.borrow_mut() = PromiseState::Resolved(Value::async_promise_from_rc(p.clone()));
        let weak = Rc::downgrade(&p);
        register_candidate(GcNode::Promise(Rc::downgrade(&p)));
        drop(p);

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 1);
        assert_eq!(weak.strong_count(), 0, "promise reclaimed");
    }

    // 6. shared subgraph reachable from TWO candidates: traced once, counts
    //    exact, collected exactly once.
    #[test]
    fn shared_subgraph_traced_once_collected_once() {
        let t1 = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        let t2 = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        let shared = Value::list(vec![
            Value::thunk_from_rc(t1.clone()),
            Value::thunk_from_rc(t2.clone()),
        ]);
        *t1.forced.borrow_mut() = Some(shared.clone());
        *t2.forced.borrow_mut() = Some(shared);
        let (w1, w2) = (Rc::downgrade(&t1), Rc::downgrade(&t2));
        register_candidate(GcNode::Thunk(Rc::downgrade(&t1)));
        register_candidate(GcNode::Thunk(Rc::downgrade(&t2)));
        drop((t1, t2));

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.candidates, 2);
        assert_eq!(stats.traced, 3, "t1 + t2 + shared list, list traced once");
        assert_eq!(stats.collected, 3);
        assert_eq!(w1.strong_count(), 0);
        assert_eq!(w2.strong_count(), 0);
    }

    // 7. pinned env bindings keep a garbage-shaped cycle alive.
    #[test]
    fn pinned_env_bindings_keep_cycle() {
        let (env, wrapper, payload, nf) = build_env_nativefn_cycle();
        let weak_nf = Rc::downgrade(&nf);
        let pin = NodePtr::of_env_bindings(&env);
        register_candidate(GcNode::ClosureFn(Rc::downgrade(&nf)));
        drop((env, wrapper, payload, nf));

        let stats = collect(&[pin], GcTrigger::Explicit);
        assert!(!stats.aborted);
        assert_eq!(stats.collected, 0, "pinned root: nothing severed");
        assert!(weak_nf.upgrade().is_some());

        // Without the pin the same graph is garbage and is reclaimed.
        let stats2 = collect(&[], GcTrigger::Explicit);
        assert!(!stats2.aborted);
        assert!(stats2.collected >= 4);
        assert!(weak_nf.upgrade().is_none());
    }

    // 8. a held borrow aborts the whole collection; nothing severed; a later
    //    collect (borrow released) reclaims.
    #[test]
    fn outstanding_borrow_aborts_collection() {
        let (env, wrapper, payload, nf) = build_env_nativefn_cycle();
        let weak_nf = Rc::downgrade(&nf);
        let bindings = env.bindings.clone();
        register_candidate(GcNode::ClosureFn(Rc::downgrade(&nf)));
        drop((env, wrapper, payload, nf));

        let guard = bindings.borrow_mut();
        let stats = collect(&[], GcTrigger::Explicit);
        assert!(stats.aborted, "borrowed bindings must abort the pass");
        assert_eq!(stats.collected, 0);
        assert!(weak_nf.upgrade().is_some(), "graph intact after abort");
        assert!(guard.contains_key(&intern("self")), "bindings untouched");
        drop(guard);
        drop(bindings);

        let stats2 = collect(&[], GcTrigger::Explicit);
        assert!(!stats2.aborted);
        assert!(stats2.collected >= 4);
        assert!(weak_nf.upgrade().is_none());
    }

    // 9. reentrancy guard: collect during a collection is a no-op.
    #[test]
    fn reentrant_collect_is_noop() {
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        *t.forced.borrow_mut() = Some(Value::thunk_from_rc(t.clone()));
        let weak = Rc::downgrade(&t);
        register_candidate(GcNode::Thunk(Rc::downgrade(&t)));
        drop(t);

        GC.with(|gc| gc.collecting.set(true));
        let stats = collect(&[], GcTrigger::Explicit);
        assert!(stats.aborted);
        assert_eq!(stats.collected, 0);
        assert!(weak.upgrade().is_some(), "no-op left the graph alone");
        assert!(maybe_collect(&[], GcTrigger::Threshold).is_none());
        GC.with(|gc| gc.collecting.set(false));

        collect(&[], GcTrigger::Explicit);
        assert_eq!(weak.strong_count(), 0);
    }

    // 10. the env-wrapper sever step is a version bump (inline-cache hygiene).
    //     Unobservable post-collect on garbage by construction, so the helper
    //     is asserted directly.
    #[test]
    fn severed_env_wrapper_bumps_version() {
        let env = Env::new();
        let v0 = env.version.get();
        sever_white_env_wrapper(&env);
        assert_eq!(env.version.get(), v0.wrapping_add(1));
    }

    // 11. duplicate edges (same NativeFn twice in one list) decrement twice.
    #[test]
    fn duplicate_edges_have_exact_multiplicity() {
        register_payload_tracer(TypeId::of::<CellPayload>(), cell_payload_tracer);
        let cell = Rc::new(TestCell {
            slot: RefCell::new(Value::NIL),
        });
        let payload = Rc::new(CellPayload { cell: cell.clone() });
        let nf = Rc::new(NativeFn::with_payload(
            "twice",
            payload.clone() as Rc<dyn Any>,
            |_, _| Ok(Value::NIL),
        ));
        *cell.slot.borrow_mut() = Value::list(vec![
            Value::native_fn_from_rc(nf.clone()),
            Value::native_fn_from_rc(nf.clone()),
        ]);
        let weak_nf = Rc::downgrade(&nf);
        register_candidate(GcNode::ClosureFn(Rc::downgrade(&nf)));
        drop((cell, payload, nf));

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 4, "nf + payload + cell + list");
        assert!(weak_nf.upgrade().is_none(), "both list slots accounted");
    }

    // 12. dead Weak entries are pruned and reported.
    #[test]
    fn dead_registry_entries_are_pruned() {
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        register_candidate(GcNode::Thunk(Rc::downgrade(&t)));
        drop(t); // acyclic: plain Rc drop reclaims it before any collect

        let stats = collect(&[], GcTrigger::Explicit);

        assert_eq!(stats.pruned, 1);
        assert_eq!(stats.candidates, 0);
        assert_eq!(stats.traced, 0);
        assert_eq!(stats.collected, 0);
    }

    // 13. threshold behavior: maybe_collect stays quiet below the threshold,
    //     and a data-birth registration that crosses it self-collects inside
    //     register_candidate — dead-entry retention between outer safe points
    //     is bounded by the threshold, not by total births.
    #[test]
    fn maybe_collect_respects_threshold() {
        assert!(
            maybe_collect(&[], GcTrigger::Threshold).is_none(),
            "empty registry: no collection"
        );
        for _ in 0..1025 {
            let t = Rc::new(Thunk {
                body: Value::NIL,
                forced: RefCell::new(None),
            });
            register_candidate(GcNode::Thunk(Rc::downgrade(&t)));
        }
        // The 1025th push crossed GC_FLOOR and ran a threshold pass inline:
        // 1024 dead entries pruned, the (then still live) current thunk kept.
        let stats = last_stats();
        assert_eq!(stats.pruned, 1024, "dead entries pruned at birth trigger");
        assert_eq!(stats.candidates, 1, "the in-scope thunk was live");
        assert!(registry_len() <= 1, "registry bounded by the trigger");
        assert!(
            maybe_collect(&[], GcTrigger::Threshold).is_none(),
            "already pruned below threshold"
        );
    }

    // trace_value multiplicity spot-checks (the arithmetic's raw material).
    #[test]
    fn trace_value_reports_exact_container_edges() {
        let nf = Rc::new(NativeFn::simple("leaf", |_| Ok(Value::NIL)));
        let nf_val = Value::native_fn_from_rc(nf);
        let list = Value::list(vec![nf_val.clone(), nf_val.clone(), Value::int(1)]);
        let mut edges = 0usize;
        assert!(trace_value(&list, &mut |e| {
            if let GcEdge::Value(v) = e {
                if value_node_ptr(v).is_some() {
                    edges += 1;
                }
            }
        }));
        assert_eq!(edges, 2, "same NativeFn twice = two edges; int = none");
    }

    #[test]
    fn trace_value_reports_forced_thunk_contents() {
        let inner = Value::list(vec![Value::int(1)]);
        let t = Rc::new(Thunk {
            body: inner.clone(),
            forced: RefCell::new(Some(inner)),
        });
        let tv = Value::thunk_from_rc(t);
        let mut edges = 0usize;
        assert!(trace_value(&tv, &mut |e| {
            if matches!(e, GcEdge::Value(_)) {
                edges += 1;
            }
        }));
        assert_eq!(edges, 2, "body + forced contents");
    }

    #[test]
    fn trace_value_aborts_on_borrowed_cell() {
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        let tv = Value::thunk_from_rc(t.clone());
        let guard = t.forced.borrow_mut();
        assert!(!trace_value(&tv, &mut |_| {}), "borrowed forced cell");
        drop(guard);
        assert!(trace_value(&tv, &mut |_| {}));
    }

    // -- Depth regressions: every collector phase runs on explicit worklists,
    //    so traversal depth must never consume Rust stack. (The Rc drop
    //    cascade of *severed* contents still recurses — pre-existing
    //    Value::drop behavior, deliberately untouched — so the fully-garbage
    //    deep test stays below drop-glue limits while the worklist tests go
    //    far past any stack budget.) --

    /// Nest `depth` single-element lists, returning a handle to every level
    /// (innermost first). Holding all levels lets a test unwind the chain
    /// outermost-first, one `Rc` release per pop, without the recursive drop
    /// cascade a plain drop of the outermost handle would trigger.
    fn deep_chain(depth: usize) -> Vec<Value> {
        let mut levels = Vec::with_capacity(depth);
        let mut v = Value::int(0);
        for _ in 0..depth {
            v = Value::list(vec![v]);
            levels.push(v.clone());
        }
        levels
    }

    /// Drop the chain without deep recursion: outermost-first, each pop
    /// releases exactly one level (its child stays alive in the vec).
    fn unwind_chain(levels: &mut Vec<Value>) {
        while levels.pop().is_some() {}
    }

    // 14a. deep LIVE structure: MarkGray + ScanBlack walk 100k levels
    //      (external handles ⇒ root count > 0 ⇒ the whole chain is
    //      re-blackened) with bounded native stack.
    #[test]
    fn deep_live_structure_traced_without_stack_overflow() {
        const DEPTH: usize = 100_000;
        let mut levels = deep_chain(DEPTH);
        let outermost = levels.last().expect("nonempty chain").clone();
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(Some(outermost)),
        });
        register_candidate(GcNode::Thunk(Rc::downgrade(&t)));

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.traced, DEPTH + 1, "thunk + every level visited");
        assert_eq!(stats.collected, 0, "externally held: everything kept");
        assert!(t.forced.borrow().is_some(), "forced cell untouched");
        t.forced.borrow_mut().take();
        unwind_chain(&mut levels);
    }

    // 14b. deep structure hanging off a GARBAGE cycle: MarkGray descends
    //      100k levels from the dead thunk; only the 2-node cycle is severed
    //      (the chain stays alive through the external handles).
    #[test]
    fn deep_garbage_cycle_collected_without_stack_overflow() {
        const DEPTH: usize = 100_000;
        let mut levels = deep_chain(DEPTH);
        let outermost = levels.last().expect("nonempty chain").clone();
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        *t.forced.borrow_mut() = Some(Value::list(vec![
            outermost,
            Value::thunk_from_rc(t.clone()),
        ]));
        let weak = Rc::downgrade(&t);
        register_candidate(GcNode::Thunk(Rc::downgrade(&t)));
        drop(t);

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, 2, "thunk + forced list; held chain kept");
        assert_eq!(weak.strong_count(), 0, "cycle reclaimed");
        assert!(
            levels.last().expect("chain kept").is_list(),
            "chain survives its garbage neighbor"
        );
        unwind_chain(&mut levels);
    }

    // 14c. fully-garbage deep chain: the whole chain goes white and the
    //      severed contents' drop cascades through the unsevered interior
    //      lists inside collect. Depth sits above the stack-overflow point
    //      of a per-level recursive MarkGray in debug builds, below the
    //      (pre-existing) recursive drop cascade's limit.
    #[test]
    fn deep_garbage_chain_fully_collected() {
        const DEPTH: usize = 3_000;
        let mut v = Value::int(0);
        for _ in 0..DEPTH {
            v = Value::list(vec![v]);
        }
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        *t.forced.borrow_mut() = Some(Value::list(vec![v, Value::thunk_from_rc(t.clone())]));
        let weak = Rc::downgrade(&t);
        register_candidate(GcNode::Thunk(Rc::downgrade(&t)));
        drop(t);

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.collected, DEPTH + 2, "chain + wrapper list + thunk");
        assert_eq!(weak.strong_count(), 0);
    }

    // 14d. garbage ring of 100k thunks linked forced→forced: MarkGray and
    //      Scan's white marking each walk the full ring depth. Every node's
    //      cell is severed, so the in-collect drops release one thunk at a
    //      time — deep-white coverage with no deep drop cascade.
    #[test]
    fn deep_thunk_ring_goes_white_without_stack_overflow() {
        const DEPTH: usize = 100_000;
        let thunks: Vec<Rc<Thunk>> = (0..DEPTH)
            .map(|_| {
                Rc::new(Thunk {
                    body: Value::NIL,
                    forced: RefCell::new(None),
                })
            })
            .collect();
        for (i, t) in thunks.iter().enumerate() {
            let next = thunks[(i + 1) % DEPTH].clone();
            *t.forced.borrow_mut() = Some(Value::thunk_from_rc(next));
        }
        let weak = Rc::downgrade(&thunks[0]);
        register_candidate(GcNode::Thunk(Rc::downgrade(&thunks[0])));
        drop(thunks);

        let stats = collect(&[], GcTrigger::Explicit);

        assert!(!stats.aborted);
        assert_eq!(stats.traced, DEPTH, "every ring node visited");
        assert_eq!(stats.collected, DEPTH, "entire ring reclaimed");
        assert_eq!(weak.strong_count(), 0);
    }

    // -- Env home-adoption registration (per-make_closure, so it must dedup;
    //    plain register_candidate is once-at-creation) --

    // 15a. register_env_candidate: first adoption registers, repeats dedup,
    //      the seen-set survives collects while the env lives and prunes
    //      when it dies. (Candidates are the home WRAPPER — the shape-E cycle
    //      is reached through it.)
    #[test]
    fn register_env_candidate_dedups_and_collects_shape_e() {
        let (env, wrapper, payload, nf) = build_env_nativefn_cycle();
        assert!(register_env_candidate(&wrapper), "first adoption registers");
        assert!(!register_env_candidate(&wrapper), "later adoptions dedup");

        let stats_live = collect(&[], GcTrigger::Explicit);
        assert!(!stats_live.aborted);
        assert_eq!(stats_live.candidates, 1, "deduped to one registration");
        assert_eq!(stats_live.collected, 0, "externally held: kept");
        assert!(
            !register_env_candidate(&wrapper),
            "seen entry survives a collect while the env lives"
        );

        let weak_bindings = Rc::downgrade(&env.bindings);
        drop((env, wrapper, payload, nf));
        let stats = collect(&[], GcTrigger::Explicit);
        assert!(!stats.aborted);
        assert_eq!(stats.collected, 4, "nf + payload + wrapper + bindings");
        assert_eq!(weak_bindings.strong_count(), 0, "env cycle reclaimed");
        assert_eq!(
            GC.with(|gc| gc.env_seen.borrow().len()),
            0,
            "seen entry pruned with the registry"
        );
        let wrapper2 = Rc::new(Env::new());
        assert!(
            register_env_candidate(&wrapper2),
            "fresh env registers anew"
        );
    }

    // 15c. register_closure_birth is the fused make_closure path: adopts the
    //      home wrapper once, registers non-exempt closures, and reports the
    //      growth threshold. A zero-upvalue-style exempt closure (None) still
    //      adopts its home, and the resulting shape-E cycle is collected via
    //      the env candidate alone.
    #[test]
    fn register_closure_birth_env_candidate_covers_exempt_closure() {
        let (env, wrapper, payload, nf) = build_env_nativefn_cycle();
        register_closure_birth(Some(&wrapper), None);
        register_closure_birth(Some(&wrapper), None);
        let weak_nf = Rc::downgrade(&nf);
        drop((env, wrapper, payload, nf));

        let stats = collect(&[], GcTrigger::Explicit);
        assert!(!stats.aborted);
        assert_eq!(stats.candidates, 1, "home adopted once, closure exempt");
        assert_eq!(
            stats.collected, 4,
            "nf + payload + wrapper + bindings via the env candidate"
        );
        assert!(weak_nf.upgrade().is_none(), "exempt closure reclaimed");
    }

    // 15b. duplicate raw registrations of one live allocation are pruned to
    //      a single entry (which keeps collecting), instead of accumulating
    //      and inflating the survivor-derived growth threshold.
    #[test]
    fn duplicate_live_registrations_pruned_to_one() {
        let t = Rc::new(Thunk {
            body: Value::NIL,
            forced: RefCell::new(None),
        });
        for _ in 0..5 {
            register_candidate(GcNode::Thunk(Rc::downgrade(&t)));
        }

        let stats = collect(&[], GcTrigger::Explicit);
        assert!(!stats.aborted);
        assert_eq!(stats.candidates, 1, "one snapshot root per allocation");
        assert_eq!(stats.pruned, 4, "duplicates removed, one entry kept");
        assert_eq!(stats.collected, 0, "live thunk kept");

        // The kept entry still collects the thunk once it becomes garbage.
        *t.forced.borrow_mut() = Some(Value::thunk_from_rc(t.clone()));
        let weak = Rc::downgrade(&t);
        drop(t);
        let stats2 = collect(&[], GcTrigger::Explicit);
        assert_eq!(stats2.candidates, 1);
        assert_eq!(stats2.collected, 1);
        assert_eq!(weak.strong_count(), 0);
    }
}
