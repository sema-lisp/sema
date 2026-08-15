use std::path::PathBuf;
use std::rc::Rc;

use sema_core::{Span, Spur, Value};

/// A compiled code object (bytecode + metadata).
#[derive(Debug, Clone)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub consts: Vec<Value>,
    pub spans: Vec<(u32, Span)>, // sparse PC → source span mapping (sorted by PC)
    pub max_stack: u16,
    pub n_locals: u16,
    pub exception_table: Vec<ExceptionEntry>,
    /// Number of per-instruction inline cache slots for global lookups.
    pub n_global_cache_slots: u16,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk {
            code: Vec::new(),
            consts: Vec::new(),
            spans: Vec::new(),
            max_stack: 0,
            n_locals: 0,
            exception_table: Vec::new(),
            n_global_cache_slots: 0,
        }
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ExceptionEntry {
    pub try_start: u32,
    pub try_end: u32,
    pub handler_pc: u32,
    pub stack_depth: u16,
    pub catch_slot: u16,
}

/// A compiled function (template for closures).
#[derive(Debug, Clone)]
pub struct Function {
    pub name: Option<Spur>,
    pub chunk: Chunk,
    pub upvalue_descs: Vec<UpvalueDesc>,
    pub upvalue_names: Vec<Spur>,
    pub arity: u16,
    pub has_rest: bool,
    /// Fixed parameter names in declaration order (excludes the rest param).
    /// Consumed by callers that bind named arguments to positions (LLM tool
    /// dispatch). Rc-shared so per-closure wrappers clone refcount-only.
    /// Not serialized (like `cache_offset`); rebuilt from `local_names` when
    /// loading a `.semac` file — empty when that reconstruction is incomplete.
    pub param_names: Rc<[Spur]>,
    pub local_names: Vec<(u16, Spur)>,
    /// Block scope of each block-introduced local, as `(slot, start_pc, end_pc)`
    /// half-open pc ranges. Used by the debugger to hide locals that are not yet
    /// bound or already out of scope at the current pc. Compile-time debug
    /// metadata only — never read during execution. Serialized as format-version-4
    /// metadata (see `serialize_function`), so it round-trips through `.semac`.
    pub local_scopes: Vec<(u16, u32, u32)>,
    pub source_file: Option<PathBuf>,
    /// Offset into the VM's inline_cache Vec where this function's cache slots begin.
    /// Assigned at VM creation time; not serialized.
    pub cache_offset: usize,
    /// Memoized "can this function's call graph suspend?" verdict for the
    /// non-suspending HOF fast path, keyed on the version fingerprint of the
    /// global env chain it was resolved against (a rebind re-analyzes). Not
    /// serialized (like `cache_offset`); computed lazily per process.
    pub suspend_cache: std::cell::Cell<Option<(u64, bool)>>,
    /// Native-code state: hot-call counter and compiled entry point, filled in
    /// by `crate::jit` when a code generator backend is installed. Not
    /// serialized (like `cache_offset`) — a `.semac` file carries bytecode, and
    /// each process decides for itself what to compile.
    pub jit: crate::jit::JitSlot,
}

/// Describes how an upvalue is captured relative to the immediately enclosing function.
#[derive(Debug, Clone, Copy)]
pub enum UpvalueDesc {
    /// Capture from the parent function's local slot.
    ParentLocal(u16),
    /// Capture from the parent function's upvalue slot.
    ParentUpvalue(u16),
}
