use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use hashbrown::HashMap as SpurMap;
use lasso::{Key, Rodeo, Spur};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::ToPrimitive;

use crate::error::SemaError;
use crate::number::Complex as SemaComplex;
use crate::number::SemaNumber;
use crate::EvalContext;

// Compile-time check: NaN-boxing requires 64-bit pointers that fit in 48-bit VA space.
// 32-bit platforms cannot use this representation (pointers don't fit the encoding).
// wasm32 is exempted because its 32-bit pointers always fit in 45 bits.
#[cfg(not(any(target_pointer_width = "64", target_arch = "wasm32")))]
compile_error!("sema-core NaN-boxed Value requires a 64-bit platform (or wasm32)");

// ── String interning ──────────────────────────────────────────────

thread_local! {
    static INTERNER: RefCell<Rodeo> = RefCell::new(Rodeo::default());
}

/// Intern a string, returning a Spur key.
pub fn intern(s: &str) -> Spur {
    INTERNER.with(|r| r.borrow_mut().get_or_intern(s))
}

/// Resolve a Spur key back to a String.
pub fn resolve(spur: Spur) -> String {
    INTERNER.with(|r| r.borrow().resolve(&spur).to_string())
}

// A `Spur` must fit in the 32-bit NaN-box payload below for the packing to round-trip.
const _: () = assert!(std::mem::size_of::<Spur>() == 4);

/// Pack an interned [`Spur`] into the 32 raw bits stored in a NaN-boxed `Value`
/// symbol/keyword payload (inverse of [`bits_to_spur`]).
///
/// The bits are the Spur's underlying `NonZeroU32` value, so two `Value`s holding
/// the same symbol compare equal by raw bits. This and [`bits_to_spur`] are the
/// single place that encodes the Spur↔bits mapping (via lasso's stable [`Key`]
/// API) — both `sema-core` and `sema-vm` go through them instead of `transmute`.
#[inline(always)]
pub fn spur_to_bits(spur: Spur) -> u32 {
    // `Key::into_usize` is offset-by-one (it returns `inner.get() - 1`); the bits
    // we store are the raw `NonZeroU32` value, i.e. `into_usize() + 1`.
    spur.into_usize() as u32 + 1
}

/// Reconstruct a [`Spur`] from the 32 raw bits stored in a NaN-boxed `Value`
/// symbol/keyword payload (inverse of [`spur_to_bits`]).
///
/// `bits` is always a value produced by [`spur_to_bits`] from a real interned
/// key, so it is non-zero and the conversion cannot fail; a zero/invalid `bits`
/// would indicate memory corruption and panics.
#[inline(always)]
pub fn bits_to_spur(bits: u32) -> Spur {
    Spur::try_from_usize((bits - 1) as usize)
        .expect("NaN-boxed symbol/keyword payload is not a valid interned key")
}

/// Resolve a Spur and call f with the &str, avoiding allocation.
pub fn with_resolved<F, R>(spur: Spur, f: F) -> R
where
    F: FnOnce(&str) -> R,
{
    INTERNER.with(|r| {
        let interner = r.borrow();
        f(interner.resolve(&spur))
    })
}

/// Return interner statistics: (count, estimated_memory_bytes).
pub fn interner_stats() -> (usize, usize) {
    INTERNER.with(|r| {
        let interner = r.borrow();
        let count = interner.len();
        let bytes = count * 16; // approximate: Spur (4 bytes) + average string data
        (count, bytes)
    })
}

// ── Gensym counter ────────────────────────────────────────────────

thread_local! {
    static GENSYM_COUNTER: Cell<u64> = const { Cell::new(0) };
}

/// Generate a unique symbol name: `<prefix>__<counter>`.
/// Used by both manual `(gensym)` and auto-gensym `foo#` in quasiquote.
/// Single shared counter prevents collisions between the two mechanisms.
pub fn next_gensym(prefix: &str) -> String {
    GENSYM_COUNTER.with(|c| {
        let val = c.get();
        c.set(val.wrapping_add(1));
        format!("{prefix}__{val}")
    })
}

/// Compare two Spurs by their resolved string content (lexicographic).
pub fn compare_spurs(a: Spur, b: Spur) -> std::cmp::Ordering {
    if a == b {
        return std::cmp::Ordering::Equal;
    }
    INTERNER.with(|r| {
        let interner = r.borrow();
        interner.resolve(&a).cmp(interner.resolve(&b))
    })
}

// ── Supporting types (unchanged public API) ───────────────────────

/// A native function callable from Sema.
pub type NativeFnInner = dyn Fn(&EvalContext, &[Value]) -> Result<Value, SemaError>;

pub struct NativeFn {
    pub name: String,
    pub func: Box<NativeFnInner>,
    pub payload: Option<Rc<dyn Any>>,
    /// True when this `NativeFn` is actually the fallback wrapper for a VM
    /// closure (a user-defined `lambda`/`fn`), not a genuine builtin. The VM
    /// represents closures as `NativeFn`s carrying a `VmClosurePayload`; this
    /// flag lets `type`/`type_name` report `:lambda` instead of `:native-fn`
    /// without sema-core/sema-stdlib needing to know the VM's payload type.
    pub is_closure: bool,
}

impl NativeFn {
    pub fn simple(
        name: impl Into<String>,
        f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            func: Box::new(move |_ctx, args| f(args)),
            payload: None,
            is_closure: false,
        }
    }

    pub fn with_ctx(
        name: impl Into<String>,
        f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            func: Box::new(f),
            payload: None,
            is_closure: false,
        }
    }

    pub fn with_payload(
        name: impl Into<String>,
        payload: Rc<dyn Any>,
        f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            func: Box::new(f),
            payload: Some(payload),
            is_closure: false,
        }
    }
}

impl fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<native-fn {}>", self.name)
    }
}

/// A user-defined lambda.
#[derive(Debug, Clone)]
pub struct Lambda {
    pub params: Vec<Spur>,
    pub rest_param: Option<Spur>,
    pub body: Vec<Value>,
    pub env: Env,
    pub name: Option<Spur>,
}

/// A macro definition.
///
/// A procedural `defmacro` uses `params`/`rest_param`/`body` and leaves
/// `syntax_rules` as `None`. An R7RS `(define-syntax name (syntax-rules ...))`
/// leaves the procedural fields empty and carries its transformer in
/// `syntax_rules`. Both share `TAG_MACRO` so env lookup, display, and GC
/// tracing treat them uniformly.
#[derive(Debug, Clone)]
pub struct Macro {
    pub params: Vec<Spur>,
    pub rest_param: Option<Spur>,
    pub body: Vec<Value>,
    pub name: Spur,
    /// `Some` for a `syntax-rules` transformer; `None` for procedural macros.
    pub syntax_rules: Option<Rc<SyntaxRules>>,
}

/// An R7RS `syntax-rules` transformer: a list of `(pattern template)` rewrite
/// rules, a set of literal identifiers matched by name, and the ellipsis symbol
/// (`...` by default, or a custom one). Patterns and templates are stored as raw
/// quoted `Value` data (list/symbol structure), so they are traced by the GC.
#[derive(Debug, Clone)]
pub struct SyntaxRules {
    pub literals: Vec<Spur>,
    pub ellipsis: Spur,
    /// Each entry is `(pattern, template)`.
    pub rules: Vec<(Value, Value)>,
}

/// A lazy promise: delay/force with memoization.
pub struct Thunk {
    pub body: Value,
    pub forced: RefCell<Option<Value>>,
}

impl fmt::Debug for Thunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.forced.borrow().is_some() {
            write!(f, "<promise (forced)>")
        } else {
            write!(f, "<promise>")
        }
    }
}

impl Clone for Thunk {
    fn clone(&self) -> Self {
        Thunk {
            body: self.body.clone(),
            forced: RefCell::new(self.forced.borrow().clone()),
        }
    }
}

/// State of an async promise/future.
///
/// `Cancelled` is a peer of `Rejected`, not a sub-kind of it: a promise that
/// the user explicitly cancels via `async/cancel` is *not* a normal rejection
/// (which a user might catch and recover from). Keeping the two distinct lets
/// `async/cancelled?` be precise without string-matching, and lets
/// `async/rejected?` honestly report `#f` for cancelled promises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromiseState {
    Pending,
    Resolved(Value),
    Rejected(String),
    Cancelled,
}

/// An async promise: represents a value that will be available in the future.
pub struct AsyncPromise {
    pub state: RefCell<PromiseState>,
    pub task_id: Cell<u64>,
}

impl fmt::Debug for AsyncPromise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.state.borrow() {
            PromiseState::Pending => write!(f, "<async-promise pending>"),
            PromiseState::Resolved(_) => write!(f, "<async-promise resolved>"),
            PromiseState::Rejected(e) => write!(f, "<async-promise rejected: {e}>"),
            PromiseState::Cancelled => write!(f, "<async-promise cancelled>"),
        }
    }
}

impl Clone for AsyncPromise {
    fn clone(&self) -> Self {
        AsyncPromise {
            state: RefCell::new(self.state.borrow().clone()),
            task_id: Cell::new(self.task_id.get()),
        }
    }
}

/// A bounded async channel for communication between coroutines.
pub struct Channel {
    pub buffer: RefCell<std::collections::VecDeque<Value>>,
    pub capacity: usize,
    pub closed: Cell<bool>,
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self.buffer.borrow().len();
        write!(f, "<channel {len}/{}>", self.capacity)
    }
}

impl Clone for Channel {
    fn clone(&self) -> Self {
        Channel {
            buffer: RefCell::new(self.buffer.borrow().clone()),
            capacity: self.capacity,
            closed: Cell::new(self.closed.get()),
        }
    }
}

/// A record: tagged product type created by define-record-type.
#[derive(Debug, Clone)]
pub struct Record {
    pub type_tag: Spur,
    pub field_names: Vec<Spur>,
    pub fields: Vec<Value>,
}

/// A message role in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// A base64-encoded image attachment.
#[derive(Debug, Clone)]
pub struct ImageAttachment {
    pub data: String,
    pub media_type: String,
}

/// A single message in a conversation.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Optional image attachments (base64-encoded).
    pub images: Vec<ImageAttachment>,
}

/// A prompt: a structured list of messages.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub messages: Vec<Message>,
}

/// A conversation: immutable history + provider config.
#[derive(Debug, Clone)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub model: String,
    pub metadata: BTreeMap<String, String>,
}

/// A tool definition for LLM function calling.
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub handler: Value,
}

/// An agent: system prompt + tools + config for autonomous loops.
#[derive(Debug, Clone)]
pub struct Agent {
    pub name: String,
    pub system: String,
    pub tools: Vec<Value>,
    pub max_turns: usize,
    pub model: String,
}

/// A multimethod: dispatch-function + method table.
/// Interior-mutable so `defmethod` can add methods after creation.
pub struct MultiMethod {
    pub name: Spur,
    pub dispatch_fn: Value,
    pub methods: RefCell<BTreeMap<Value, Value>>,
    pub default: RefCell<Option<Value>>,
}

impl fmt::Debug for MultiMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<multimethod {}>", resolve(self.name))
    }
}

/// Trait for stream implementations (files, buffers, serial ports, etc.).
/// All methods take `&self` — interior mutability is handled by the implementation.
pub trait SemaStream: fmt::Debug {
    fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError>;
    fn write(&self, data: &[u8]) -> Result<usize, SemaError>;
    fn available(&self) -> Result<bool, SemaError> {
        Ok(false)
    }
    fn flush(&self) -> Result<(), SemaError> {
        Ok(())
    }
    fn close(&self) -> Result<(), SemaError> {
        Ok(())
    }
    fn is_readable(&self) -> bool {
        true
    }
    fn is_writable(&self) -> bool {
        true
    }
    fn stream_type(&self) -> &'static str;
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Sized wrapper around `dyn SemaStream` for NaN-boxing (thin pointer via Rc<StreamBox>).
/// Tracks closed state centrally so all impls get close-guarding for free.
pub struct StreamBox {
    inner: RefCell<Box<dyn SemaStream>>,
    closed: Cell<bool>,
}

impl StreamBox {
    pub fn new(s: impl SemaStream + 'static) -> Self {
        StreamBox {
            inner: RefCell::new(Box::new(s)),
            closed: Cell::new(false),
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError> {
        if self.closed.get() {
            return Err(SemaError::eval("stream/read: stream is closed"));
        }
        self.inner.borrow().read(buf)
    }

    pub fn write(&self, data: &[u8]) -> Result<usize, SemaError> {
        if self.closed.get() {
            return Err(SemaError::eval("stream/write: stream is closed"));
        }
        self.inner.borrow().write(data)
    }

    pub fn flush(&self) -> Result<(), SemaError> {
        if self.closed.get() {
            return Err(SemaError::eval("stream/flush: stream is closed"));
        }
        self.inner.borrow().flush()
    }

    pub fn close(&self) -> Result<(), SemaError> {
        if self.closed.get() {
            return Ok(()); // double-close is a no-op
        }
        self.inner.borrow().close()?;
        self.closed.set(true);
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    pub fn is_readable(&self) -> bool {
        !self.closed.get() && self.inner.borrow().is_readable()
    }

    pub fn is_writable(&self) -> bool {
        !self.closed.get() && self.inner.borrow().is_writable()
    }

    pub fn available(&self) -> Result<bool, SemaError> {
        if self.closed.get() {
            return Ok(false);
        }
        self.inner.borrow().available()
    }

    pub fn stream_type(&self) -> &'static str {
        self.inner.borrow().stream_type()
    }

    pub fn borrow_inner(&self) -> std::cell::Ref<'_, Box<dyn SemaStream>> {
        self.inner.borrow()
    }
}

impl fmt::Debug for StreamBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<stream:{}>", self.stream_type())
    }
}

impl Clone for MultiMethod {
    fn clone(&self) -> Self {
        MultiMethod {
            name: self.name,
            dispatch_fn: self.dispatch_fn.clone(),
            methods: RefCell::new(self.methods.borrow().clone()),
            default: RefCell::new(self.default.borrow().clone()),
        }
    }
}

// ── NaN-boxing constants ──────────────────────────────────────────

// IEEE 754 double layout:
//   bit 63:     sign
//   bits 62-52: exponent (11 bits)
//   bits 51-0:  mantissa (52 bits), bit 51 = quiet NaN bit
//
// Boxed (non-float) values use: sign=1, exp=all 1s, quiet=1
//   Then bits 50-45 = TAG (6 bits), bits 44-0 = PAYLOAD (45 bits)

/// Mask for checking if a value is boxed (sign + exponent + quiet bit)
const BOX_MASK: u64 = 0xFFF8_0000_0000_0000;

/// The 45-bit payload mask
const PAYLOAD_MASK: u64 = (1u64 << 45) - 1; // 0x1FFF_FFFF_FFFF

/// Sign-extension bit for 45-bit signed integers
const INT_SIGN_BIT: u64 = 1u64 << 44;

/// 6-bit mask for extracting the tag from a boxed value (bits 50-45).
const TAG_MASK_6BIT: u64 = 0x3F;

/// Canonical quiet NaN (sign=0) — used for NaN float values to avoid collision with boxed
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

// Tags (6 bits, encoded in bits 50-45)
const TAG_NIL: u64 = 0;
const TAG_FALSE: u64 = 1;
const TAG_TRUE: u64 = 2;
const TAG_INT_SMALL: u64 = 3;
const TAG_CHAR: u64 = 4;
const TAG_SYMBOL: u64 = 5;
const TAG_KEYWORD: u64 = 6;
const TAG_INT_BIG: u64 = 7;
const TAG_STRING: u64 = 8;
const TAG_LIST: u64 = 9;
const TAG_VECTOR: u64 = 10;
const TAG_MAP: u64 = 11;
const TAG_HASHMAP: u64 = 12;
const TAG_LAMBDA: u64 = 13;
const TAG_MACRO: u64 = 14;
pub const TAG_NATIVE_FN: u64 = 15;
const TAG_PROMPT: u64 = 16;
const TAG_MESSAGE: u64 = 17;
const TAG_CONVERSATION: u64 = 18;
const TAG_TOOL_DEF: u64 = 19;
const TAG_AGENT: u64 = 20;
const TAG_THUNK: u64 = 21;
const TAG_RECORD: u64 = 22;
const TAG_BYTEVECTOR: u64 = 23;
const TAG_MULTIMETHOD: u64 = 24;
const TAG_STREAM: u64 = 25;
const TAG_F64_ARRAY: u64 = 26;
const TAG_I64_ARRAY: u64 = 27;
const TAG_ASYNC_PROMISE: u64 = 28;
const TAG_CHANNEL: u64 = 29;
const TAG_BIGINT: u64 = 30;
const TAG_RATIONAL: u64 = 31;
const TAG_COMPLEX: u64 = 32;

/// Small-int range: [-2^44, 2^44 - 1] = [-17_592_186_044_416, +17_592_186_044_415]
const SMALL_INT_MIN: i64 = -(1i64 << 44);
const SMALL_INT_MAX: i64 = (1i64 << 44) - 1;

// ── Public NaN-boxing constants for VM use ────────────────────────

/// Tag + box combined mask: upper 19 bits (sign + exponent + quiet + 6-bit tag).
pub const NAN_TAG_MASK: u64 = BOX_MASK | (TAG_MASK_6BIT << 45); // 0xFFFF_E000_0000_0000

/// The expected upper bits for a small int value: BOX_MASK | (TAG_INT_SMALL << 45).
pub const NAN_INT_SMALL_PATTERN: u64 = BOX_MASK | (TAG_INT_SMALL << 45);

/// Public payload mask (45 bits).
pub const NAN_PAYLOAD_MASK: u64 = PAYLOAD_MASK;

/// Sign bit within the 45-bit payload (bit 44) — for sign-extending small ints.
pub const NAN_INT_SIGN_BIT: u64 = INT_SIGN_BIT;

/// Number of payload bits in NaN-boxed values (45).
pub const NAN_PAYLOAD_BITS: u32 = 45;

// ── Helpers for encoding/decoding ─────────────────────────────────

#[inline(always)]
fn make_boxed(tag: u64, payload: u64) -> u64 {
    BOX_MASK | (tag << 45) | (payload & PAYLOAD_MASK)
}

#[inline(always)]
fn is_boxed(bits: u64) -> bool {
    (bits & BOX_MASK) == BOX_MASK
}

#[inline(always)]
fn get_tag(bits: u64) -> u64 {
    (bits >> 45) & TAG_MASK_6BIT
}

#[inline(always)]
fn get_payload(bits: u64) -> u64 {
    bits & PAYLOAD_MASK
}

#[inline(always)]
fn ptr_to_payload(ptr: *const u8) -> u64 {
    let raw = ptr as u64;
    debug_assert!(raw & 0x7 == 0, "pointer not 8-byte aligned: 0x{:x}", raw);
    debug_assert!(
        raw >> 48 == 0,
        "pointer exceeds 48-bit VA space: 0x{:x}",
        raw
    );
    raw >> 3
}

#[inline(always)]
fn payload_to_ptr(payload: u64) -> *const u8 {
    (payload << 3) as *const u8
}

// ── ValueView: pattern-matching enum ──────────────────────────────

/// A view of a NaN-boxed Value for pattern matching.
/// Returned by `Value::view()`. Heap types hold Rc (refcount bumped).
pub enum ValueView {
    Nil,
    Bool(bool),
    Int(i64),
    BigInt(Rc<BigInt>),
    Rational(Rc<BigRational>),
    Complex(Rc<SemaComplex>),
    Float(f64),
    String(Rc<String>),
    Symbol(Spur),
    Keyword(Spur),
    Char(char),
    List(Rc<Vec<Value>>),
    Vector(Rc<Vec<Value>>),
    Map(Rc<BTreeMap<Value, Value>>),
    HashMap(Rc<hashbrown::HashMap<Value, Value>>),
    Lambda(Rc<Lambda>),
    Macro(Rc<Macro>),
    NativeFn(Rc<NativeFn>),
    Prompt(Rc<Prompt>),
    Message(Rc<Message>),
    Conversation(Rc<Conversation>),
    ToolDef(Rc<ToolDefinition>),
    Agent(Rc<Agent>),
    Thunk(Rc<Thunk>),
    Record(Rc<Record>),
    Bytevector(Rc<Vec<u8>>),
    MultiMethod(Rc<MultiMethod>),
    Stream(Rc<StreamBox>),
    F64Array(Rc<Vec<f64>>),
    I64Array(Rc<Vec<i64>>),
    AsyncPromise(Rc<AsyncPromise>),
    Channel(Rc<Channel>),
}

/// A borrowing view of a `Value` — like `ValueView` but returns references
/// instead of cloning `Rc`s, avoiding refcount mutations on every comparison,
/// hash, and ordering operation.
pub enum ValueViewRef<'a> {
    Nil,
    Bool(bool),
    Int(i64),
    BigInt(&'a BigInt),
    Rational(&'a BigRational),
    Complex(&'a SemaComplex),
    Float(f64),
    String(&'a str),
    Symbol(Spur),
    Keyword(Spur),
    Char(char),
    List(&'a [Value]),
    Vector(&'a [Value]),
    Map(&'a BTreeMap<Value, Value>),
    HashMap(&'a hashbrown::HashMap<Value, Value>),
    Lambda(&'a Lambda),
    Macro(&'a Macro),
    NativeFn(&'a NativeFn),
    Prompt(&'a Prompt),
    Message(&'a Message),
    Conversation(&'a Conversation),
    ToolDef(&'a ToolDefinition),
    Agent(&'a Agent),
    Thunk(&'a Thunk),
    Record(&'a Record),
    Bytevector(&'a [u8]),
    MultiMethod(&'a MultiMethod),
    Stream(&'a StreamBox),
    F64Array(&'a [f64]),
    I64Array(&'a [i64]),
    AsyncPromise(&'a AsyncPromise),
    Channel(&'a Channel),
}

// ── The NaN-boxed Value type ──────────────────────────────────────

/// The core Value type for all Sema data.
/// NaN-boxed: stored as 8 bytes. Floats stored directly,
/// everything else encoded in quiet-NaN payload space.
#[repr(transparent)]
pub struct Value(u64);

// ── Constructors ──────────────────────────────────────────────────

impl Value {
    // -- Immediate constructors --

    pub const NIL: Value = Value(make_boxed_const(TAG_NIL, 0));
    pub const TRUE: Value = Value(make_boxed_const(TAG_TRUE, 0));
    pub const FALSE: Value = Value(make_boxed_const(TAG_FALSE, 0));

    #[inline(always)]
    pub fn nil() -> Value {
        Value::NIL
    }

    #[inline(always)]
    pub fn bool(b: bool) -> Value {
        if b {
            Value::TRUE
        } else {
            Value::FALSE
        }
    }

    #[inline(always)]
    pub fn int(n: i64) -> Value {
        if (SMALL_INT_MIN..=SMALL_INT_MAX).contains(&n) {
            // Encode as small int (45-bit two's complement)
            let payload = (n as u64) & PAYLOAD_MASK;
            Value(make_boxed(TAG_INT_SMALL, payload))
        } else {
            // Out of range: heap-allocate
            let rc = Rc::new(n);
            let ptr = Rc::into_raw(rc) as *const u8;
            Value(make_boxed(TAG_INT_BIG, ptr_to_payload(ptr)))
        }
    }

    #[inline(always)]
    pub fn float(f: f64) -> Value {
        let bits = f.to_bits();
        if f.is_nan() {
            // Canonicalize NaN to avoid collision with boxed patterns
            Value(CANONICAL_NAN)
        } else {
            // Check: a non-NaN float could still have the BOX_MASK pattern
            // This happens for negative infinity and some subnormals — but
            // negative infinity is 0xFFF0_0000_0000_0000 which does NOT match
            // BOX_MASK (0xFFF8...) because bit 51 (quiet) is 0.
            // In IEEE 754, the only values with all exponent bits set AND quiet bit set
            // are quiet NaNs, which we've already canonicalized above.
            debug_assert!(
                !is_boxed(bits),
                "non-NaN float collides with boxed pattern: {:?} = 0x{:016x}",
                f,
                bits
            );
            Value(bits)
        }
    }

    #[inline(always)]
    pub fn char(c: char) -> Value {
        Value(make_boxed(TAG_CHAR, c as u64))
    }

    #[inline(always)]
    pub fn symbol_from_spur(spur: Spur) -> Value {
        Value(make_boxed(TAG_SYMBOL, spur_to_bits(spur) as u64))
    }

    pub fn symbol(s: &str) -> Value {
        Value::symbol_from_spur(intern(s))
    }

    #[inline(always)]
    pub fn keyword_from_spur(spur: Spur) -> Value {
        Value(make_boxed(TAG_KEYWORD, spur_to_bits(spur) as u64))
    }

    pub fn keyword(s: &str) -> Value {
        Value::keyword_from_spur(intern(s))
    }

    // -- Heap constructors --

    fn from_rc_ptr<T>(tag: u64, rc: Rc<T>) -> Value {
        let ptr = Rc::into_raw(rc) as *const u8;
        Value(make_boxed(tag, ptr_to_payload(ptr)))
    }

    /// Construct an integer of any magnitude, normalizing to the tightest
    /// representation: values in i64 range become a fixnum/int-big, larger
    /// values are heap-boxed under `TAG_BIGINT`.
    pub fn from_bigint(n: BigInt) -> Value {
        match n.to_i64() {
            Some(i) => Value::int(i),
            None => Value::from_rc_ptr(TAG_BIGINT, Rc::new(n)),
        }
    }

    /// Construct an exact rational, normalizing integer-valued rationals
    /// (e.g. `6/3`) down to the tightest integer representation.
    pub fn rational(r: BigRational) -> Value {
        if r.is_integer() {
            Value::from_bigint(r.to_integer())
        } else {
            Value::from_rc_ptr(TAG_RATIONAL, Rc::new(r))
        }
    }

    /// Construct a complex number from its real/imaginary tower components,
    /// normalizing an exact-zero imaginary part down to the real part alone.
    pub fn complex(re: SemaNumber, im: SemaNumber) -> Value {
        Value::from_number(SemaNumber::Complex(Box::new(SemaComplex { re, im })))
    }

    pub fn string(s: &str) -> Value {
        Value::from_rc_ptr(TAG_STRING, Rc::new(s.to_string()))
    }

    pub fn string_from_rc(rc: Rc<String>) -> Value {
        Value::from_rc_ptr(TAG_STRING, rc)
    }

    pub fn list(v: Vec<Value>) -> Value {
        Value::from_rc_ptr(TAG_LIST, Rc::new(v))
    }

    pub fn list_from_rc(rc: Rc<Vec<Value>>) -> Value {
        Value::from_rc_ptr(TAG_LIST, rc)
    }

    pub fn vector(v: Vec<Value>) -> Value {
        Value::from_rc_ptr(TAG_VECTOR, Rc::new(v))
    }

    pub fn vector_from_rc(rc: Rc<Vec<Value>>) -> Value {
        Value::from_rc_ptr(TAG_VECTOR, rc)
    }

    pub fn map(m: BTreeMap<Value, Value>) -> Value {
        Value::from_rc_ptr(TAG_MAP, Rc::new(m))
    }

    pub fn map_from_rc(rc: Rc<BTreeMap<Value, Value>>) -> Value {
        Value::from_rc_ptr(TAG_MAP, rc)
    }

    pub fn hashmap(entries: Vec<(Value, Value)>) -> Value {
        let map: hashbrown::HashMap<Value, Value> = entries.into_iter().collect();
        Value::from_rc_ptr(TAG_HASHMAP, Rc::new(map))
    }

    pub fn hashmap_from_rc(rc: Rc<hashbrown::HashMap<Value, Value>>) -> Value {
        Value::from_rc_ptr(TAG_HASHMAP, rc)
    }

    pub fn lambda(l: Lambda) -> Value {
        Value::from_rc_ptr(TAG_LAMBDA, Rc::new(l))
    }

    pub fn lambda_from_rc(rc: Rc<Lambda>) -> Value {
        Value::from_rc_ptr(TAG_LAMBDA, rc)
    }

    pub fn macro_val(m: Macro) -> Value {
        Value::from_rc_ptr(TAG_MACRO, Rc::new(m))
    }

    pub fn macro_from_rc(rc: Rc<Macro>) -> Value {
        Value::from_rc_ptr(TAG_MACRO, rc)
    }

    pub fn native_fn(f: NativeFn) -> Value {
        Value::from_rc_ptr(TAG_NATIVE_FN, Rc::new(f))
    }

    pub fn native_fn_from_rc(rc: Rc<NativeFn>) -> Value {
        Value::from_rc_ptr(TAG_NATIVE_FN, rc)
    }

    pub fn prompt(p: Prompt) -> Value {
        Value::from_rc_ptr(TAG_PROMPT, Rc::new(p))
    }

    pub fn prompt_from_rc(rc: Rc<Prompt>) -> Value {
        Value::from_rc_ptr(TAG_PROMPT, rc)
    }

    pub fn message(m: Message) -> Value {
        Value::from_rc_ptr(TAG_MESSAGE, Rc::new(m))
    }

    pub fn message_from_rc(rc: Rc<Message>) -> Value {
        Value::from_rc_ptr(TAG_MESSAGE, rc)
    }

    pub fn conversation(c: Conversation) -> Value {
        Value::from_rc_ptr(TAG_CONVERSATION, Rc::new(c))
    }

    pub fn conversation_from_rc(rc: Rc<Conversation>) -> Value {
        Value::from_rc_ptr(TAG_CONVERSATION, rc)
    }

    pub fn tool_def(t: ToolDefinition) -> Value {
        Value::from_rc_ptr(TAG_TOOL_DEF, Rc::new(t))
    }

    pub fn tool_def_from_rc(rc: Rc<ToolDefinition>) -> Value {
        Value::from_rc_ptr(TAG_TOOL_DEF, rc)
    }

    pub fn agent(a: Agent) -> Value {
        Value::from_rc_ptr(TAG_AGENT, Rc::new(a))
    }

    pub fn agent_from_rc(rc: Rc<Agent>) -> Value {
        Value::from_rc_ptr(TAG_AGENT, rc)
    }

    pub fn thunk(t: Thunk) -> Value {
        let rc = Rc::new(t);
        // Cold data-cycle constructor (CORE-2, plan §5.2): a thunk can carry a
        // closure-free cycle through its `forced` cell, so every fresh thunk
        // is a collector candidate. `from_rc` wrappers are exempt — they wrap
        // allocations registered at their own creation site.
        crate::cycle::register_candidate(crate::cycle::GcNode::Thunk(Rc::downgrade(&rc)));
        Value::from_rc_ptr(TAG_THUNK, rc)
    }

    pub fn thunk_from_rc(rc: Rc<Thunk>) -> Value {
        Value::from_rc_ptr(TAG_THUNK, rc)
    }

    pub fn record(r: Record) -> Value {
        Value::from_rc_ptr(TAG_RECORD, Rc::new(r))
    }

    pub fn record_from_rc(rc: Rc<Record>) -> Value {
        Value::from_rc_ptr(TAG_RECORD, rc)
    }

    pub fn bytevector(bytes: Vec<u8>) -> Value {
        Value::from_rc_ptr(TAG_BYTEVECTOR, Rc::new(bytes))
    }

    pub fn bytevector_from_rc(rc: Rc<Vec<u8>>) -> Value {
        Value::from_rc_ptr(TAG_BYTEVECTOR, rc)
    }

    pub fn f64_array(data: Vec<f64>) -> Value {
        Value::from_rc_ptr(TAG_F64_ARRAY, Rc::new(data))
    }

    pub fn f64_array_from_rc(rc: Rc<Vec<f64>>) -> Value {
        Value::from_rc_ptr(TAG_F64_ARRAY, rc)
    }

    pub fn i64_array(data: Vec<i64>) -> Value {
        Value::from_rc_ptr(TAG_I64_ARRAY, Rc::new(data))
    }

    pub fn i64_array_from_rc(rc: Rc<Vec<i64>>) -> Value {
        Value::from_rc_ptr(TAG_I64_ARRAY, rc)
    }

    pub fn multimethod(m: MultiMethod) -> Value {
        let rc = Rc::new(m);
        // Cold data-cycle constructor (CORE-2): the method table / default
        // cells can close a closure-free cycle (e.g. a method value that is
        // the multimethod itself).
        crate::cycle::register_candidate(crate::cycle::GcNode::MultiMethod(Rc::downgrade(&rc)));
        Value::from_rc_ptr(TAG_MULTIMETHOD, rc)
    }

    pub fn multimethod_from_rc(rc: Rc<MultiMethod>) -> Value {
        Value::from_rc_ptr(TAG_MULTIMETHOD, rc)
    }

    pub fn stream(s: impl SemaStream + 'static) -> Value {
        Value::from_rc_ptr(TAG_STREAM, Rc::new(StreamBox::new(s)))
    }

    pub fn stream_from_rc(rc: Rc<StreamBox>) -> Value {
        Value::from_rc_ptr(TAG_STREAM, rc)
    }

    pub fn async_promise(promise: AsyncPromise) -> Value {
        let rc = Rc::new(promise);
        // Cold data-cycle constructor (CORE-2): a resolved promise can close a
        // closure-free cycle through its state cell (e.g. resolved to a channel
        // that buffers the promise). The scheduler's raw `Rc::new(AsyncPromise…)`
        // spawn sites register their own candidates before wrapping via
        // `async_promise_from_rc`.
        crate::cycle::register_candidate(crate::cycle::GcNode::Promise(Rc::downgrade(&rc)));
        Value::from_rc_ptr(TAG_ASYNC_PROMISE, rc)
    }
    pub fn async_promise_from_rc(rc: Rc<AsyncPromise>) -> Value {
        Value::from_rc_ptr(TAG_ASYNC_PROMISE, rc)
    }
    pub fn channel(ch: Channel) -> Value {
        let rc = Rc::new(ch);
        // Cold data-cycle constructor (CORE-2): the buffer can hold values
        // that reach back to the channel with no closure on the cycle.
        crate::cycle::register_candidate(crate::cycle::GcNode::Channel(Rc::downgrade(&rc)));
        Value::from_rc_ptr(TAG_CHANNEL, rc)
    }
    pub fn channel_from_rc(rc: Rc<Channel>) -> Value {
        Value::from_rc_ptr(TAG_CHANNEL, rc)
    }
}

// Const-compatible boxed encoding (no function calls)
const fn make_boxed_const(tag: u64, payload: u64) -> u64 {
    BOX_MASK | (tag << 45) | (payload & PAYLOAD_MASK)
}

// ── Accessors ─────────────────────────────────────────────────────

impl Value {
    /// Get the raw bits (for debugging/testing).
    #[inline(always)]
    pub fn raw_bits(&self) -> u64 {
        self.0
    }

    /// Construct a Value from raw NaN-boxed bits.
    ///
    /// # Safety
    ///
    /// Caller must ensure `bits` represents a valid NaN-boxed value.
    /// For immediate types (nil, bool, int, symbol, keyword, char), this is always safe.
    /// For heap-pointer types, the encoded pointer must be valid and have its Rc ownership
    /// accounted for (i.e., the caller must ensure the refcount is correct).
    #[inline(always)]
    pub unsafe fn from_raw_bits(bits: u64) -> Value {
        Value(bits)
    }

    /// Get the NaN-boxing tag of a boxed value (0-63).
    /// Returns `None` for non-boxed values (floats).
    #[inline(always)]
    pub fn raw_tag(&self) -> Option<u64> {
        if is_boxed(self.0) {
            Some(get_tag(self.0))
        } else {
            None
        }
    }

    /// Borrow the underlying NativeFn without bumping the Rc refcount.
    /// SAFETY: The returned reference is valid as long as this Value is alive.
    #[inline(always)]
    pub fn as_native_fn_ref(&self) -> Option<&NativeFn> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_NATIVE_FN {
            Some(unsafe { self.borrow_ref::<NativeFn>() })
        } else {
            None
        }
    }

    /// Check if this is a float (non-boxed).
    #[inline(always)]
    pub fn is_float(&self) -> bool {
        !is_boxed(self.0)
    }

    /// Recover an Rc<T> pointer from the payload WITHOUT consuming ownership.
    /// This increments the refcount (returns a new Rc).
    #[inline(always)]
    unsafe fn get_rc<T>(&self) -> Rc<T> {
        let payload = get_payload(self.0);
        let ptr = payload_to_ptr(payload) as *const T;
        Rc::increment_strong_count(ptr);
        Rc::from_raw(ptr)
    }

    /// Borrow the underlying T from a heap-tagged Value.
    /// SAFETY: caller must ensure the tag matches and T is correct.
    #[inline(always)]
    unsafe fn borrow_ref<T>(&self) -> &T {
        let payload = get_payload(self.0);
        let ptr = payload_to_ptr(payload) as *const T;
        &*ptr
    }

    /// Pattern-match friendly view of this value.
    /// For heap types, this bumps the Rc refcount.
    pub fn view(&self) -> ValueView {
        if !is_boxed(self.0) {
            return ValueView::Float(f64::from_bits(self.0));
        }
        let tag = get_tag(self.0);
        match tag {
            TAG_NIL => ValueView::Nil,
            TAG_FALSE => ValueView::Bool(false),
            TAG_TRUE => ValueView::Bool(true),
            TAG_INT_SMALL => {
                let payload = get_payload(self.0);
                let val = if payload & INT_SIGN_BIT != 0 {
                    (payload | !PAYLOAD_MASK) as i64
                } else {
                    payload as i64
                };
                ValueView::Int(val)
            }
            TAG_CHAR => {
                let payload = get_payload(self.0);
                ValueView::Char(unsafe { char::from_u32_unchecked(payload as u32) })
            }
            TAG_SYMBOL => {
                let payload = get_payload(self.0);
                ValueView::Symbol(bits_to_spur(payload as u32))
            }
            TAG_KEYWORD => {
                let payload = get_payload(self.0);
                ValueView::Keyword(bits_to_spur(payload as u32))
            }
            TAG_INT_BIG => {
                let val = unsafe { *self.borrow_ref::<i64>() };
                ValueView::Int(val)
            }
            TAG_BIGINT => ValueView::BigInt(unsafe { self.get_rc::<BigInt>() }),
            TAG_RATIONAL => ValueView::Rational(unsafe { self.get_rc::<BigRational>() }),
            TAG_COMPLEX => ValueView::Complex(unsafe { self.get_rc::<SemaComplex>() }),
            // SAFETY: every TAG_X arm below calls `get_rc::<T>()` where T matches the
            // type stored by the corresponding Value::<x>() constructor. The Clone and
            // Drop impls elsewhere in this file mirror this dispatch table — when adding
            // a new tag here, update both. The tag check above each branch is what makes
            // the transmute inside get_rc sound.
            TAG_STRING => ValueView::String(unsafe { self.get_rc::<String>() }),
            TAG_LIST => ValueView::List(unsafe { self.get_rc::<Vec<Value>>() }),
            TAG_VECTOR => ValueView::Vector(unsafe { self.get_rc::<Vec<Value>>() }),
            TAG_MAP => ValueView::Map(unsafe { self.get_rc::<BTreeMap<Value, Value>>() }),
            TAG_HASHMAP => {
                ValueView::HashMap(unsafe { self.get_rc::<hashbrown::HashMap<Value, Value>>() })
            }
            TAG_LAMBDA => ValueView::Lambda(unsafe { self.get_rc::<Lambda>() }),
            TAG_MACRO => ValueView::Macro(unsafe { self.get_rc::<Macro>() }),
            TAG_NATIVE_FN => ValueView::NativeFn(unsafe { self.get_rc::<NativeFn>() }),
            TAG_PROMPT => ValueView::Prompt(unsafe { self.get_rc::<Prompt>() }),
            TAG_MESSAGE => ValueView::Message(unsafe { self.get_rc::<Message>() }),
            TAG_CONVERSATION => ValueView::Conversation(unsafe { self.get_rc::<Conversation>() }),
            TAG_TOOL_DEF => ValueView::ToolDef(unsafe { self.get_rc::<ToolDefinition>() }),
            TAG_AGENT => ValueView::Agent(unsafe { self.get_rc::<Agent>() }),
            TAG_THUNK => ValueView::Thunk(unsafe { self.get_rc::<Thunk>() }),
            TAG_RECORD => ValueView::Record(unsafe { self.get_rc::<Record>() }),
            TAG_BYTEVECTOR => ValueView::Bytevector(unsafe { self.get_rc::<Vec<u8>>() }),
            TAG_MULTIMETHOD => ValueView::MultiMethod(unsafe { self.get_rc::<MultiMethod>() }),
            TAG_STREAM => ValueView::Stream(unsafe { self.get_rc::<StreamBox>() }),
            TAG_F64_ARRAY => ValueView::F64Array(unsafe { self.get_rc::<Vec<f64>>() }),
            TAG_I64_ARRAY => ValueView::I64Array(unsafe { self.get_rc::<Vec<i64>>() }),
            TAG_ASYNC_PROMISE => ValueView::AsyncPromise(unsafe { self.get_rc::<AsyncPromise>() }),
            TAG_CHANNEL => ValueView::Channel(unsafe { self.get_rc::<Channel>() }),
            _ => unreachable!("invalid NaN-boxed tag: {}", tag),
        }
    }

    /// Borrowing view — like `view()` but returns references instead of
    /// bumping Rc refcounts.  Use this in hot paths like `PartialEq`,
    /// `Hash`, `Ord`, and `Display`.
    #[inline(always)]
    pub fn view_ref(&self) -> ValueViewRef<'_> {
        if !is_boxed(self.0) {
            return ValueViewRef::Float(f64::from_bits(self.0));
        }
        let tag = get_tag(self.0);
        match tag {
            TAG_NIL => ValueViewRef::Nil,
            TAG_FALSE => ValueViewRef::Bool(false),
            TAG_TRUE => ValueViewRef::Bool(true),
            TAG_INT_SMALL => {
                let payload = get_payload(self.0);
                let val = if payload & INT_SIGN_BIT != 0 {
                    (payload | !PAYLOAD_MASK) as i64
                } else {
                    payload as i64
                };
                ValueViewRef::Int(val)
            }
            TAG_CHAR => {
                let payload = get_payload(self.0);
                ValueViewRef::Char(unsafe { char::from_u32_unchecked(payload as u32) })
            }
            TAG_SYMBOL => {
                let payload = get_payload(self.0);
                ValueViewRef::Symbol(bits_to_spur(payload as u32))
            }
            TAG_KEYWORD => {
                let payload = get_payload(self.0);
                ValueViewRef::Keyword(bits_to_spur(payload as u32))
            }
            TAG_INT_BIG => {
                let val = unsafe { *self.borrow_ref::<i64>() };
                ValueViewRef::Int(val)
            }
            TAG_BIGINT => ValueViewRef::BigInt(unsafe { self.borrow_ref::<BigInt>() }),
            TAG_RATIONAL => ValueViewRef::Rational(unsafe { self.borrow_ref::<BigRational>() }),
            TAG_COMPLEX => ValueViewRef::Complex(unsafe { self.borrow_ref::<SemaComplex>() }),
            // SAFETY: same tag/type correspondence as view() — see the
            // comment in view().  borrow_ref returns &T without touching
            // the refcount.
            TAG_STRING => ValueViewRef::String(unsafe { self.borrow_ref::<String>() }),
            TAG_LIST => ValueViewRef::List(unsafe { self.borrow_ref::<Vec<Value>>() }),
            TAG_VECTOR => ValueViewRef::Vector(unsafe { self.borrow_ref::<Vec<Value>>() }),
            TAG_MAP => ValueViewRef::Map(unsafe { self.borrow_ref::<BTreeMap<Value, Value>>() }),
            TAG_HASHMAP => ValueViewRef::HashMap(unsafe {
                self.borrow_ref::<hashbrown::HashMap<Value, Value>>()
            }),
            TAG_LAMBDA => ValueViewRef::Lambda(unsafe { self.borrow_ref::<Lambda>() }),
            TAG_MACRO => ValueViewRef::Macro(unsafe { self.borrow_ref::<Macro>() }),
            TAG_NATIVE_FN => ValueViewRef::NativeFn(unsafe { self.borrow_ref::<NativeFn>() }),
            TAG_PROMPT => ValueViewRef::Prompt(unsafe { self.borrow_ref::<Prompt>() }),
            TAG_MESSAGE => ValueViewRef::Message(unsafe { self.borrow_ref::<Message>() }),
            TAG_CONVERSATION => {
                ValueViewRef::Conversation(unsafe { self.borrow_ref::<Conversation>() })
            }
            TAG_TOOL_DEF => ValueViewRef::ToolDef(unsafe { self.borrow_ref::<ToolDefinition>() }),
            TAG_AGENT => ValueViewRef::Agent(unsafe { self.borrow_ref::<Agent>() }),
            TAG_THUNK => ValueViewRef::Thunk(unsafe { self.borrow_ref::<Thunk>() }),
            TAG_RECORD => ValueViewRef::Record(unsafe { self.borrow_ref::<Record>() }),
            TAG_BYTEVECTOR => ValueViewRef::Bytevector(unsafe { self.borrow_ref::<Vec<u8>>() }),
            TAG_MULTIMETHOD => {
                ValueViewRef::MultiMethod(unsafe { self.borrow_ref::<MultiMethod>() })
            }
            TAG_STREAM => ValueViewRef::Stream(unsafe { self.borrow_ref::<StreamBox>() }),
            TAG_F64_ARRAY => ValueViewRef::F64Array(unsafe { self.borrow_ref::<Vec<f64>>() }),
            TAG_I64_ARRAY => ValueViewRef::I64Array(unsafe { self.borrow_ref::<Vec<i64>>() }),
            TAG_ASYNC_PROMISE => {
                ValueViewRef::AsyncPromise(unsafe { self.borrow_ref::<AsyncPromise>() })
            }
            TAG_CHANNEL => ValueViewRef::Channel(unsafe { self.borrow_ref::<Channel>() }),
            _ => unreachable!("invalid NaN-boxed tag: {}", tag),
        }
    }

    /// Data pointer of the heap allocation behind this value — the cycle
    /// collector's node identity. `None` for floats and immediates.
    pub(crate) fn heap_ptr(&self) -> Option<*const u8> {
        if !is_boxed(self.0) {
            return None;
        }
        match get_tag(self.0) {
            TAG_NIL | TAG_FALSE | TAG_TRUE | TAG_INT_SMALL | TAG_CHAR | TAG_SYMBOL
            | TAG_KEYWORD => None,
            _ => Some(payload_to_ptr(get_payload(self.0))),
        }
    }

    /// `Rc::strong_count` of the heap allocation behind this value, read
    /// without perturbing the count (the collector's trial-deletion seed).
    /// `None` for floats and immediates.
    pub(crate) fn heap_strong_count(&self) -> Option<usize> {
        /// Read the strong count of the `Rc<T>` whose data pointer is `ptr`.
        ///
        /// SAFETY: caller must pass the data pointer of a live `Rc<T>` with the
        /// correct `T` for the value's tag (same tag→type table as `view()`,
        /// `Clone`, and `Drop`). `ManuallyDrop` prevents the reconstructed `Rc`
        /// from decrementing the count it merely reads — the established
        /// pattern from `with_hashmap_mut_if_unique`.
        unsafe fn count_at<T>(ptr: *const u8) -> usize {
            let rc = std::mem::ManuallyDrop::new(unsafe { Rc::from_raw(ptr as *const T) });
            Rc::strong_count(&rc)
        }
        let ptr = self.heap_ptr()?;
        let n = unsafe {
            match get_tag(self.0) {
                TAG_INT_BIG => count_at::<i64>(ptr),
                TAG_BIGINT => count_at::<BigInt>(ptr),
                TAG_RATIONAL => count_at::<BigRational>(ptr),
                TAG_COMPLEX => count_at::<SemaComplex>(ptr),
                TAG_STRING => count_at::<String>(ptr),
                TAG_LIST | TAG_VECTOR => count_at::<Vec<Value>>(ptr),
                TAG_MAP => count_at::<BTreeMap<Value, Value>>(ptr),
                TAG_HASHMAP => count_at::<hashbrown::HashMap<Value, Value>>(ptr),
                TAG_LAMBDA => count_at::<Lambda>(ptr),
                TAG_MACRO => count_at::<Macro>(ptr),
                TAG_NATIVE_FN => count_at::<NativeFn>(ptr),
                TAG_PROMPT => count_at::<Prompt>(ptr),
                TAG_MESSAGE => count_at::<Message>(ptr),
                TAG_CONVERSATION => count_at::<Conversation>(ptr),
                TAG_TOOL_DEF => count_at::<ToolDefinition>(ptr),
                TAG_AGENT => count_at::<Agent>(ptr),
                TAG_THUNK => count_at::<Thunk>(ptr),
                TAG_RECORD => count_at::<Record>(ptr),
                TAG_BYTEVECTOR => count_at::<Vec<u8>>(ptr),
                TAG_MULTIMETHOD => count_at::<MultiMethod>(ptr),
                TAG_STREAM => count_at::<StreamBox>(ptr),
                TAG_F64_ARRAY => count_at::<Vec<f64>>(ptr),
                TAG_I64_ARRAY => count_at::<Vec<i64>>(ptr),
                TAG_ASYNC_PROMISE => count_at::<AsyncPromise>(ptr),
                TAG_CHANNEL => count_at::<Channel>(ptr),
                _ => unreachable!("invalid heap tag in heap_strong_count"),
            }
        };
        Some(n)
    }

    // -- Typed accessors (ergonomic, avoid full view match) --

    #[inline(always)]
    pub fn type_name(&self) -> &'static str {
        if !is_boxed(self.0) {
            return "float";
        }
        match get_tag(self.0) {
            TAG_NIL => "nil",
            TAG_FALSE | TAG_TRUE => "bool",
            TAG_INT_SMALL | TAG_INT_BIG | TAG_BIGINT => "int",
            TAG_RATIONAL => "rational",
            TAG_COMPLEX => "complex",
            TAG_CHAR => "char",
            TAG_SYMBOL => "symbol",
            TAG_KEYWORD => "keyword",
            TAG_STRING => "string",
            TAG_LIST => "list",
            TAG_VECTOR => "vector",
            TAG_MAP => "map",
            TAG_HASHMAP => "hashmap",
            TAG_LAMBDA => "lambda",
            TAG_MACRO => "macro",
            TAG_NATIVE_FN => "native-fn",
            TAG_PROMPT => "prompt",
            TAG_MESSAGE => "message",
            TAG_CONVERSATION => "conversation",
            TAG_TOOL_DEF => "tool",
            TAG_AGENT => "agent",
            TAG_THUNK => "promise",
            TAG_RECORD => "record",
            TAG_BYTEVECTOR => "bytevector",
            TAG_MULTIMETHOD => "multimethod",
            TAG_STREAM => "stream",
            TAG_F64_ARRAY => "f64-array",
            TAG_I64_ARRAY => "i64-array",
            TAG_ASYNC_PROMISE => "async-promise",
            TAG_CHANNEL => "channel",
            _ => "unknown",
        }
    }

    #[inline(always)]
    pub fn is_nil(&self) -> bool {
        self.0 == Value::NIL.0
    }

    #[inline(always)]
    pub fn is_truthy(&self) -> bool {
        self.0 != Value::NIL.0 && self.0 != Value::FALSE.0
    }

    #[inline(always)]
    pub fn is_falsy(&self) -> bool {
        !self.is_truthy()
    }

    #[inline(always)]
    pub fn is_bool(&self) -> bool {
        self.0 == Value::TRUE.0 || self.0 == Value::FALSE.0
    }

    #[inline(always)]
    pub fn is_int(&self) -> bool {
        is_boxed(self.0) && matches!(get_tag(self.0), TAG_INT_SMALL | TAG_INT_BIG)
    }

    #[inline(always)]
    pub fn is_bigint(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_BIGINT
    }

    #[inline(always)]
    pub fn is_rational(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_RATIONAL
    }

    #[inline(always)]
    pub fn is_complex(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_COMPLEX
    }

    #[inline(always)]
    pub fn is_symbol(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_SYMBOL
    }

    #[inline(always)]
    pub fn is_keyword(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_KEYWORD
    }

    #[inline(always)]
    pub fn is_string(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_STRING
    }

    #[inline(always)]
    pub fn is_list(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_LIST
    }

    #[inline(always)]
    pub fn is_pair(&self) -> bool {
        if let Some(items) = self.as_list() {
            !items.is_empty()
        } else {
            false
        }
    }

    #[inline(always)]
    pub fn is_vector(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_VECTOR
    }

    #[inline(always)]
    pub fn is_map(&self) -> bool {
        is_boxed(self.0) && matches!(get_tag(self.0), TAG_MAP | TAG_HASHMAP)
    }

    #[inline(always)]
    pub fn is_lambda(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_LAMBDA
    }

    #[inline(always)]
    pub fn is_native_fn(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_NATIVE_FN
    }

    #[inline(always)]
    pub fn is_thunk(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_THUNK
    }

    #[inline(always)]
    pub fn is_async_promise(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_ASYNC_PROMISE
    }
    #[inline(always)]
    pub fn is_channel(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_CHANNEL
    }

    #[inline(always)]
    pub fn is_record(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_RECORD
    }

    #[inline(always)]
    pub fn as_int(&self) -> Option<i64> {
        if !is_boxed(self.0) {
            return None;
        }
        match get_tag(self.0) {
            TAG_INT_SMALL => {
                let payload = get_payload(self.0);
                let val = if payload & INT_SIGN_BIT != 0 {
                    (payload | !PAYLOAD_MASK) as i64
                } else {
                    payload as i64
                };
                Some(val)
            }
            TAG_INT_BIG => Some(unsafe { *self.borrow_ref::<i64>() }),
            _ => None,
        }
    }

    /// Lift any integer Value (fixnum, int-big, or bignum) to `BigInt`.
    /// `None` for non-integers.
    pub fn as_bigint(&self) -> Option<BigInt> {
        match self.view_ref() {
            ValueViewRef::Int(n) => Some(BigInt::from(n)),
            ValueViewRef::BigInt(n) => Some(n.clone()),
            _ => None,
        }
    }

    /// Lift any exact Value (fixnum, bignum, or rational) to `BigRational`.
    /// `None` for non-exact-numeric Values (including floats).
    pub fn as_rational(&self) -> Option<BigRational> {
        match self.view_ref() {
            ValueViewRef::Int(n) => Some(BigRational::from(BigInt::from(n))),
            ValueViewRef::BigInt(n) => Some(BigRational::from(n.clone())),
            ValueViewRef::Rational(r) => Some(r.clone()),
            _ => None,
        }
    }

    /// Lift any numeric Value into the tower type for arithmetic. `None` for
    /// non-numbers.
    pub fn as_number(&self) -> Option<SemaNumber> {
        match self.view_ref() {
            ValueViewRef::Int(n) => Some(SemaNumber::from_i64(n)),
            ValueViewRef::BigInt(n) => Some(SemaNumber::Integer(n.clone())),
            ValueViewRef::Rational(r) => Some(SemaNumber::Rational(r.clone())),
            ValueViewRef::Complex(c) => Some(SemaNumber::Complex(Box::new(c.clone()))),
            ValueViewRef::Float(f) => Some(SemaNumber::Real(f)),
            _ => None,
        }
    }

    /// Lower a tower number to the tightest Value.
    pub fn from_number(n: SemaNumber) -> Value {
        match n.normalize() {
            SemaNumber::Integer(big) => Value::from_bigint(big),
            SemaNumber::Rational(r) => Value::rational(r),
            SemaNumber::Real(f) => Value::float(f),
            SemaNumber::Complex(c) => Value::from_rc_ptr(TAG_COMPLEX, Rc::new(*c)),
        }
    }

    /// Lift a complex Value to the tower's `Complex` component pair. `None`
    /// for non-complex Values.
    pub fn as_complex(&self) -> Option<SemaComplex> {
        if let ValueViewRef::Complex(c) = self.view_ref() {
            Some(c.clone())
        } else {
            None
        }
    }

    /// Convert a user-supplied integer to a `usize` index/count, rejecting
    /// non-integers and negative values. Centralizes the negativity guard that
    /// `list/take`, `list/drop`, `string/repeat` (and the Pattern-A audit sites)
    /// all need — a bare `as usize` would wrap a negative `i64` to a huge value
    /// and trigger an OOM allocation or out-of-bounds panic.
    pub fn as_index(&self, name: &str) -> Result<usize, SemaError> {
        let n = self.as_int().ok_or_else(|| {
            SemaError::type_error("int", self.type_name())
                .with_hint(format!("{name}: argument must be an integer"))
        })?;
        if n < 0 {
            return Err(SemaError::eval(format!(
                "{name}: expected a non-negative integer, got {n}"
            ))
            .with_hint("pass 0 or a positive integer"));
        }
        Ok(n as usize)
    }

    #[inline(always)]
    pub fn as_float(&self) -> Option<f64> {
        if !is_boxed(self.0) {
            return Some(f64::from_bits(self.0));
        }
        match get_tag(self.0) {
            TAG_INT_SMALL => {
                let payload = get_payload(self.0);
                let val = if payload & INT_SIGN_BIT != 0 {
                    (payload | !PAYLOAD_MASK) as i64
                } else {
                    payload as i64
                };
                Some(val as f64)
            }
            TAG_INT_BIG => Some(unsafe { *self.borrow_ref::<i64>() } as f64),
            _ => None,
        }
    }

    #[inline(always)]
    pub fn as_bool(&self) -> Option<bool> {
        if self.0 == Value::TRUE.0 {
            Some(true)
        } else if self.0 == Value::FALSE.0 {
            Some(false)
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn as_str(&self) -> Option<&str> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_STRING {
            Some(unsafe { self.borrow_ref::<String>() })
        } else {
            None
        }
    }

    pub fn as_string_rc(&self) -> Option<Rc<String>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_STRING {
            Some(unsafe { self.get_rc::<String>() })
        } else {
            None
        }
    }

    pub fn as_symbol(&self) -> Option<String> {
        self.as_symbol_spur().map(resolve)
    }

    pub fn as_symbol_spur(&self) -> Option<Spur> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_SYMBOL {
            let payload = get_payload(self.0);
            Some(bits_to_spur(payload as u32))
        } else {
            None
        }
    }

    pub fn as_keyword(&self) -> Option<String> {
        self.as_keyword_spur().map(resolve)
    }

    pub fn as_keyword_spur(&self) -> Option<Spur> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_KEYWORD {
            let payload = get_payload(self.0);
            Some(bits_to_spur(payload as u32))
        } else {
            None
        }
    }

    pub fn as_char(&self) -> Option<char> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_CHAR {
            let payload = get_payload(self.0);
            char::from_u32(payload as u32)
        } else {
            None
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_LIST {
            Some(unsafe { self.borrow_ref::<Vec<Value>>() })
        } else {
            None
        }
    }

    pub fn as_list_rc(&self) -> Option<Rc<Vec<Value>>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_LIST {
            Some(unsafe { self.get_rc::<Vec<Value>>() })
        } else {
            None
        }
    }

    /// Returns the contents as a slice if this is a list OR a vector.
    pub fn as_seq(&self) -> Option<&[Value]> {
        self.as_list().or_else(|| self.as_vector())
    }

    pub fn as_vector(&self) -> Option<&[Value]> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_VECTOR {
            Some(unsafe { self.borrow_ref::<Vec<Value>>() })
        } else {
            None
        }
    }

    pub fn as_vector_rc(&self) -> Option<Rc<Vec<Value>>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_VECTOR {
            Some(unsafe { self.get_rc::<Vec<Value>>() })
        } else {
            None
        }
    }

    pub fn as_map_rc(&self) -> Option<Rc<BTreeMap<Value, Value>>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_MAP {
            Some(unsafe { self.get_rc::<BTreeMap<Value, Value>>() })
        } else {
            None
        }
    }

    pub fn as_hashmap_rc(&self) -> Option<Rc<hashbrown::HashMap<Value, Value>>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_HASHMAP {
            Some(unsafe { self.get_rc::<hashbrown::HashMap<Value, Value>>() })
        } else {
            None
        }
    }

    /// Borrow the underlying HashMap without bumping the Rc refcount.
    #[inline(always)]
    pub fn as_hashmap_ref(&self) -> Option<&hashbrown::HashMap<Value, Value>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_HASHMAP {
            Some(unsafe { self.borrow_ref::<hashbrown::HashMap<Value, Value>>() })
        } else {
            None
        }
    }

    /// Borrow the underlying BTreeMap without bumping the Rc refcount.
    #[inline(always)]
    pub fn as_map_ref(&self) -> Option<&BTreeMap<Value, Value>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_MAP {
            Some(unsafe { self.borrow_ref::<BTreeMap<Value, Value>>() })
        } else {
            None
        }
    }

    /// If this is a hashmap with refcount==1, mutate it in place.
    /// Returns `None` if not a hashmap or if shared (refcount > 1).
    /// SAFETY: relies on no other references to the inner data existing.
    #[inline(always)]
    pub fn with_hashmap_mut_if_unique<R>(
        &self,
        f: impl FnOnce(&mut hashbrown::HashMap<Value, Value>) -> R,
    ) -> Option<R> {
        if !is_boxed(self.0) || get_tag(self.0) != TAG_HASHMAP {
            return None;
        }
        let payload = get_payload(self.0);
        let ptr = payload_to_ptr(payload) as *const hashbrown::HashMap<Value, Value>;
        let rc = std::mem::ManuallyDrop::new(unsafe { Rc::from_raw(ptr) });
        if Rc::strong_count(&rc) != 1 {
            return None;
        }
        // strong_count==1: we are the sole owner, safe to mutate
        let ptr_mut = ptr as *mut hashbrown::HashMap<Value, Value>;
        Some(f(unsafe { &mut *ptr_mut }))
    }

    /// If this is a map (BTreeMap) with refcount==1, mutate it in place.
    /// Returns `None` if not a map or if shared (refcount > 1).
    #[inline(always)]
    pub fn with_map_mut_if_unique<R>(
        &self,
        f: impl FnOnce(&mut BTreeMap<Value, Value>) -> R,
    ) -> Option<R> {
        if !is_boxed(self.0) || get_tag(self.0) != TAG_MAP {
            return None;
        }
        let payload = get_payload(self.0);
        let ptr = payload_to_ptr(payload) as *const BTreeMap<Value, Value>;
        let rc = std::mem::ManuallyDrop::new(unsafe { Rc::from_raw(ptr) });
        if Rc::strong_count(&rc) != 1 {
            return None;
        }
        let ptr_mut = ptr as *mut BTreeMap<Value, Value>;
        Some(f(unsafe { &mut *ptr_mut }))
    }

    /// Consume this Value and extract the inner Rc without a refcount bump.
    /// Returns `Err(self)` if not a hashmap.
    pub fn into_hashmap_rc(self) -> Result<Rc<hashbrown::HashMap<Value, Value>>, Value> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_HASHMAP {
            let payload = get_payload(self.0);
            let ptr = payload_to_ptr(payload) as *const hashbrown::HashMap<Value, Value>;
            // Prevent Drop from decrementing the refcount — we're taking ownership
            std::mem::forget(self);
            Ok(unsafe { Rc::from_raw(ptr) })
        } else {
            Err(self)
        }
    }

    /// Consume this Value and extract the inner Rc without a refcount bump.
    /// Returns `Err(self)` if not a map.
    pub fn into_map_rc(self) -> Result<Rc<BTreeMap<Value, Value>>, Value> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_MAP {
            let payload = get_payload(self.0);
            let ptr = payload_to_ptr(payload) as *const BTreeMap<Value, Value>;
            std::mem::forget(self);
            Ok(unsafe { Rc::from_raw(ptr) })
        } else {
            Err(self)
        }
    }

    pub fn as_lambda_rc(&self) -> Option<Rc<Lambda>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_LAMBDA {
            Some(unsafe { self.get_rc::<Lambda>() })
        } else {
            None
        }
    }

    pub fn as_macro_rc(&self) -> Option<Rc<Macro>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_MACRO {
            Some(unsafe { self.get_rc::<Macro>() })
        } else {
            None
        }
    }

    pub fn as_native_fn_rc(&self) -> Option<Rc<NativeFn>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_NATIVE_FN {
            Some(unsafe { self.get_rc::<NativeFn>() })
        } else {
            None
        }
    }

    pub fn as_thunk_rc(&self) -> Option<Rc<Thunk>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_THUNK {
            Some(unsafe { self.get_rc::<Thunk>() })
        } else {
            None
        }
    }

    pub fn as_record(&self) -> Option<&Record> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_RECORD {
            Some(unsafe { self.borrow_ref::<Record>() })
        } else {
            None
        }
    }

    pub fn as_record_rc(&self) -> Option<Rc<Record>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_RECORD {
            Some(unsafe { self.get_rc::<Record>() })
        } else {
            None
        }
    }

    pub fn as_bytevector(&self) -> Option<&[u8]> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_BYTEVECTOR {
            Some(unsafe { self.borrow_ref::<Vec<u8>>() })
        } else {
            None
        }
    }

    pub fn as_bytevector_rc(&self) -> Option<Rc<Vec<u8>>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_BYTEVECTOR {
            Some(unsafe { self.get_rc::<Vec<u8>>() })
        } else {
            None
        }
    }

    pub fn as_f64_array(&self) -> Option<&[f64]> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_F64_ARRAY {
            Some(unsafe { self.borrow_ref::<Vec<f64>>() })
        } else {
            None
        }
    }

    pub fn as_f64_array_rc(&self) -> Option<Rc<Vec<f64>>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_F64_ARRAY {
            Some(unsafe { self.get_rc::<Vec<f64>>() })
        } else {
            None
        }
    }

    pub fn as_i64_array(&self) -> Option<&[i64]> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_I64_ARRAY {
            Some(unsafe { self.borrow_ref::<Vec<i64>>() })
        } else {
            None
        }
    }

    pub fn as_i64_array_rc(&self) -> Option<Rc<Vec<i64>>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_I64_ARRAY {
            Some(unsafe { self.get_rc::<Vec<i64>>() })
        } else {
            None
        }
    }

    pub fn as_stream(&self) -> Option<&StreamBox> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_STREAM {
            Some(unsafe { self.borrow_ref::<StreamBox>() })
        } else {
            None
        }
    }

    pub fn as_stream_rc(&self) -> Option<Rc<StreamBox>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_STREAM {
            Some(unsafe { self.get_rc::<StreamBox>() })
        } else {
            None
        }
    }

    pub fn as_prompt_rc(&self) -> Option<Rc<Prompt>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_PROMPT {
            Some(unsafe { self.get_rc::<Prompt>() })
        } else {
            None
        }
    }

    pub fn as_message_rc(&self) -> Option<Rc<Message>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_MESSAGE {
            Some(unsafe { self.get_rc::<Message>() })
        } else {
            None
        }
    }

    pub fn as_conversation_rc(&self) -> Option<Rc<Conversation>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_CONVERSATION {
            Some(unsafe { self.get_rc::<Conversation>() })
        } else {
            None
        }
    }

    pub fn as_tool_def_rc(&self) -> Option<Rc<ToolDefinition>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_TOOL_DEF {
            Some(unsafe { self.get_rc::<ToolDefinition>() })
        } else {
            None
        }
    }

    pub fn as_agent_rc(&self) -> Option<Rc<Agent>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_AGENT {
            Some(unsafe { self.get_rc::<Agent>() })
        } else {
            None
        }
    }

    pub fn as_multimethod_rc(&self) -> Option<Rc<MultiMethod>> {
        if is_boxed(self.0) && get_tag(self.0) == TAG_MULTIMETHOD {
            Some(unsafe { self.get_rc::<MultiMethod>() })
        } else {
            None
        }
    }
}

// ── Clone ─────────────────────────────────────────────────────────

impl Clone for Value {
    #[inline(always)]
    fn clone(&self) -> Self {
        if !is_boxed(self.0) {
            // Float: trivial copy
            return Value(self.0);
        }
        let tag = get_tag(self.0);
        match tag {
            // Immediates: trivial copy
            TAG_NIL | TAG_FALSE | TAG_TRUE | TAG_INT_SMALL | TAG_CHAR | TAG_SYMBOL
            | TAG_KEYWORD => Value(self.0),
            // Heap pointers: increment refcount
            _ => {
                let payload = get_payload(self.0);
                let ptr = payload_to_ptr(payload);
                // Increment refcount based on type
                unsafe {
                    match tag {
                        TAG_INT_BIG => Rc::increment_strong_count(ptr as *const i64),
                        TAG_BIGINT => Rc::increment_strong_count(ptr as *const BigInt),
                        TAG_RATIONAL => Rc::increment_strong_count(ptr as *const BigRational),
                        TAG_COMPLEX => Rc::increment_strong_count(ptr as *const SemaComplex),
                        TAG_STRING => Rc::increment_strong_count(ptr as *const String),
                        TAG_LIST | TAG_VECTOR => {
                            Rc::increment_strong_count(ptr as *const Vec<Value>)
                        }
                        TAG_MAP => Rc::increment_strong_count(ptr as *const BTreeMap<Value, Value>),
                        TAG_HASHMAP => Rc::increment_strong_count(
                            ptr as *const hashbrown::HashMap<Value, Value>,
                        ),
                        TAG_LAMBDA => Rc::increment_strong_count(ptr as *const Lambda),
                        TAG_MACRO => Rc::increment_strong_count(ptr as *const Macro),
                        TAG_NATIVE_FN => Rc::increment_strong_count(ptr as *const NativeFn),
                        TAG_PROMPT => Rc::increment_strong_count(ptr as *const Prompt),
                        TAG_MESSAGE => Rc::increment_strong_count(ptr as *const Message),
                        TAG_CONVERSATION => Rc::increment_strong_count(ptr as *const Conversation),
                        TAG_TOOL_DEF => Rc::increment_strong_count(ptr as *const ToolDefinition),
                        TAG_AGENT => Rc::increment_strong_count(ptr as *const Agent),
                        TAG_THUNK => Rc::increment_strong_count(ptr as *const Thunk),
                        TAG_RECORD => Rc::increment_strong_count(ptr as *const Record),
                        TAG_BYTEVECTOR => Rc::increment_strong_count(ptr as *const Vec<u8>),
                        TAG_MULTIMETHOD => Rc::increment_strong_count(ptr as *const MultiMethod),
                        TAG_STREAM => Rc::increment_strong_count(ptr as *const StreamBox),
                        TAG_F64_ARRAY => Rc::increment_strong_count(ptr as *const Vec<f64>),
                        TAG_I64_ARRAY => Rc::increment_strong_count(ptr as *const Vec<i64>),
                        TAG_ASYNC_PROMISE => Rc::increment_strong_count(ptr as *const AsyncPromise),
                        TAG_CHANNEL => Rc::increment_strong_count(ptr as *const Channel),
                        _ => unreachable!("invalid heap tag in clone: {}", tag),
                    }
                }
                Value(self.0)
            }
        }
    }
}

// ── Drop ──────────────────────────────────────────────────────────

impl Drop for Value {
    #[inline(always)]
    fn drop(&mut self) {
        if !is_boxed(self.0) {
            return; // Float
        }
        let tag = get_tag(self.0);
        match tag {
            // Immediates: nothing to free
            TAG_NIL | TAG_FALSE | TAG_TRUE | TAG_INT_SMALL | TAG_CHAR | TAG_SYMBOL
            | TAG_KEYWORD => {}
            // Heap pointers: drop the Rc
            _ => {
                let payload = get_payload(self.0);
                let ptr = payload_to_ptr(payload);
                unsafe {
                    match tag {
                        TAG_INT_BIG => drop(Rc::from_raw(ptr as *const i64)),
                        TAG_BIGINT => drop(Rc::from_raw(ptr as *const BigInt)),
                        TAG_RATIONAL => drop(Rc::from_raw(ptr as *const BigRational)),
                        TAG_COMPLEX => drop(Rc::from_raw(ptr as *const SemaComplex)),
                        TAG_STRING => drop(Rc::from_raw(ptr as *const String)),
                        TAG_LIST | TAG_VECTOR => drop(Rc::from_raw(ptr as *const Vec<Value>)),
                        TAG_MAP => drop(Rc::from_raw(ptr as *const BTreeMap<Value, Value>)),
                        TAG_HASHMAP => {
                            drop(Rc::from_raw(ptr as *const hashbrown::HashMap<Value, Value>))
                        }
                        TAG_LAMBDA => drop(Rc::from_raw(ptr as *const Lambda)),
                        TAG_MACRO => drop(Rc::from_raw(ptr as *const Macro)),
                        TAG_NATIVE_FN => drop(Rc::from_raw(ptr as *const NativeFn)),
                        TAG_PROMPT => drop(Rc::from_raw(ptr as *const Prompt)),
                        TAG_MESSAGE => drop(Rc::from_raw(ptr as *const Message)),
                        TAG_CONVERSATION => drop(Rc::from_raw(ptr as *const Conversation)),
                        TAG_TOOL_DEF => drop(Rc::from_raw(ptr as *const ToolDefinition)),
                        TAG_AGENT => drop(Rc::from_raw(ptr as *const Agent)),
                        TAG_THUNK => drop(Rc::from_raw(ptr as *const Thunk)),
                        TAG_RECORD => drop(Rc::from_raw(ptr as *const Record)),
                        TAG_BYTEVECTOR => drop(Rc::from_raw(ptr as *const Vec<u8>)),
                        TAG_MULTIMETHOD => drop(Rc::from_raw(ptr as *const MultiMethod)),
                        TAG_STREAM => drop(Rc::from_raw(ptr as *const StreamBox)),
                        TAG_F64_ARRAY => drop(Rc::from_raw(ptr as *const Vec<f64>)),
                        TAG_I64_ARRAY => drop(Rc::from_raw(ptr as *const Vec<i64>)),
                        TAG_ASYNC_PROMISE => drop(Rc::from_raw(ptr as *const AsyncPromise)),
                        TAG_CHANNEL => drop(Rc::from_raw(ptr as *const Channel)),
                        _ => {} // unreachable, but don't panic in drop
                    }
                }
            }
        }
    }
}

// ── PartialEq / Eq ────────────────────────────────────────────────

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: identical bits
        if self.0 == other.0 {
            // For floats, NaN != NaN per IEEE, but our canonical NaN is unique,
            // so identical bits means equal for all types.
            // Exception: need to handle -0.0 == +0.0
            if !is_boxed(self.0) {
                let f = f64::from_bits(self.0);
                // NaN check: if both are canonical NaN (same bits), we say not equal
                if f.is_nan() {
                    return false;
                }
                return true;
            }
            return true;
        }
        // Different bits: could still be equal for heap types or -0.0/+0.0
        match (self.view_ref(), other.view_ref()) {
            (ValueViewRef::Nil, ValueViewRef::Nil) => true,
            (ValueViewRef::Bool(a), ValueViewRef::Bool(b)) => a == b,
            (ValueViewRef::Int(a), ValueViewRef::Int(b)) => a == b,
            (ValueViewRef::BigInt(a), ValueViewRef::BigInt(b)) => a == b,
            (ValueViewRef::Rational(a), ValueViewRef::Rational(b)) => a == b,
            (ValueViewRef::Complex(a), ValueViewRef::Complex(b)) => a.re == b.re && a.im == b.im,
            (ValueViewRef::Float(a), ValueViewRef::Float(b)) => a == b,
            (ValueViewRef::String(a), ValueViewRef::String(b)) => a == b,
            (ValueViewRef::Symbol(a), ValueViewRef::Symbol(b)) => a == b,
            (ValueViewRef::Keyword(a), ValueViewRef::Keyword(b)) => a == b,
            (ValueViewRef::Char(a), ValueViewRef::Char(b)) => a == b,
            (ValueViewRef::List(a), ValueViewRef::List(b)) => a == b,
            (ValueViewRef::Vector(a), ValueViewRef::Vector(b)) => a == b,
            (ValueViewRef::Map(a), ValueViewRef::Map(b)) => a == b,
            (ValueViewRef::HashMap(a), ValueViewRef::HashMap(b)) => a == b,
            (ValueViewRef::Record(a), ValueViewRef::Record(b)) => {
                a.type_tag == b.type_tag && a.fields == b.fields
            }
            (ValueViewRef::Bytevector(a), ValueViewRef::Bytevector(b)) => a == b,
            (ValueViewRef::F64Array(a), ValueViewRef::F64Array(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(x, y)| x.to_bits() == y.to_bits())
            }
            (ValueViewRef::I64Array(a), ValueViewRef::I64Array(b)) => a == b,
            (ValueViewRef::Stream(a), ValueViewRef::Stream(b)) => std::ptr::eq(a, b),
            (ValueViewRef::AsyncPromise(a), ValueViewRef::AsyncPromise(b)) => std::ptr::eq(a, b),
            (ValueViewRef::Channel(a), ValueViewRef::Channel(b)) => std::ptr::eq(a, b),
            _ => false,
        }
    }
}

impl Eq for Value {}

// ── Hash ──────────────────────────────────────────────────────────

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.view_ref() {
            ValueViewRef::Nil => 0u8.hash(state),
            ValueViewRef::Bool(b) => {
                1u8.hash(state);
                b.hash(state);
            }
            ValueViewRef::Int(n) => {
                2u8.hash(state);
                n.hash(state);
            }
            ValueViewRef::BigInt(n) => {
                30u8.hash(state);
                n.hash(state);
            }
            ValueViewRef::Rational(r) => {
                31u8.hash(state);
                r.hash(state);
            }
            ValueViewRef::Complex(c) => {
                32u8.hash(state);
                c.re.hash(state);
                c.im.hash(state);
            }
            ValueViewRef::Float(f) => {
                3u8.hash(state);
                let bits = if f == 0.0 { 0u64 } else { f.to_bits() };
                bits.hash(state);
            }
            ValueViewRef::String(s) => {
                4u8.hash(state);
                s.hash(state);
            }
            ValueViewRef::Symbol(s) => {
                5u8.hash(state);
                s.hash(state);
            }
            ValueViewRef::Keyword(s) => {
                6u8.hash(state);
                s.hash(state);
            }
            ValueViewRef::Char(c) => {
                7u8.hash(state);
                c.hash(state);
            }
            ValueViewRef::List(l) => {
                8u8.hash(state);
                l.hash(state);
            }
            ValueViewRef::Vector(v) => {
                9u8.hash(state);
                v.hash(state);
            }
            ValueViewRef::Record(r) => {
                10u8.hash(state);
                r.type_tag.hash(state);
                r.fields.hash(state);
            }
            ValueViewRef::Bytevector(bv) => {
                11u8.hash(state);
                bv.hash(state);
            }
            ValueViewRef::F64Array(arr) => {
                26u8.hash(state);
                for v in arr.iter() {
                    v.to_bits().hash(state);
                }
            }
            ValueViewRef::I64Array(arr) => {
                27u8.hash(state);
                arr.hash(state);
            }
            ValueViewRef::Stream(s) => {
                25u8.hash(state);
                (s as *const _ as usize).hash(state);
            }
            ValueViewRef::AsyncPromise(p) => {
                28u8.hash(state);
                (p as *const _ as usize).hash(state);
            }
            ValueViewRef::Channel(c) => {
                29u8.hash(state);
                (c as *const _ as usize).hash(state);
            }
            _ => {}
        }
    }
}

// ── Ord ───────────────────────────────────────────────────────────

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        fn type_order(v: &Value) -> u8 {
            match v.view_ref() {
                ValueViewRef::Nil => 0,
                ValueViewRef::Bool(_) => 1,
                ValueViewRef::Int(_) | ValueViewRef::BigInt(_) => 2,
                ValueViewRef::Float(_) => 3,
                ValueViewRef::Char(_) => 4,
                ValueViewRef::String(_) => 5,
                ValueViewRef::Symbol(_) => 6,
                ValueViewRef::Keyword(_) => 7,
                ValueViewRef::List(_) => 8,
                ValueViewRef::Vector(_) => 9,
                ValueViewRef::Map(_) => 10,
                ValueViewRef::HashMap(_) => 11,
                ValueViewRef::Record(_) => 12,
                ValueViewRef::Bytevector(_) => 13,
                ValueViewRef::F64Array(_) => 14,
                ValueViewRef::I64Array(_) => 15,
                ValueViewRef::Stream(_) => 16,
                ValueViewRef::Rational(_) => 18,
                ValueViewRef::Complex(_) => 19,
                _ => 17,
            }
        }
        match (self.view_ref(), other.view_ref()) {
            (ValueViewRef::Nil, ValueViewRef::Nil) => Ordering::Equal,
            (ValueViewRef::Bool(a), ValueViewRef::Bool(b)) => a.cmp(&b),
            (ValueViewRef::Int(a), ValueViewRef::Int(b)) => a.cmp(&b),
            (ValueViewRef::BigInt(a), ValueViewRef::BigInt(b)) => a.cmp(b),
            (ValueViewRef::Int(a), ValueViewRef::BigInt(b)) => BigInt::from(a).cmp(b),
            (ValueViewRef::BigInt(a), ValueViewRef::Int(b)) => a.cmp(&BigInt::from(b)),
            (ValueViewRef::Rational(a), ValueViewRef::Rational(b)) => a.cmp(b),
            (ValueViewRef::Complex(a), ValueViewRef::Complex(b)) => {
                a.re.cmp(&b.re).then_with(|| a.im.cmp(&b.im))
            }
            (ValueViewRef::Float(a), ValueViewRef::Float(b)) => {
                // Normalize signed zeros so -0.0 and +0.0 are the same map key:
                // Hash already collapses them and `=` treats them equal, but
                // total_cmp otherwise orders -0.0 < +0.0, silently splitting a
                // BTreeMap key. (NaN handling is unaffected.)
                let norm = |f: f64| if f == 0.0 { 0.0 } else { f };
                norm(a).total_cmp(&norm(b))
            }
            (ValueViewRef::String(a), ValueViewRef::String(b)) => a.cmp(b),
            (ValueViewRef::Symbol(a), ValueViewRef::Symbol(b)) => compare_spurs(a, b),
            (ValueViewRef::Keyword(a), ValueViewRef::Keyword(b)) => compare_spurs(a, b),
            (ValueViewRef::Char(a), ValueViewRef::Char(b)) => a.cmp(&b),
            (ValueViewRef::List(a), ValueViewRef::List(b)) => a.cmp(b),
            (ValueViewRef::Vector(a), ValueViewRef::Vector(b)) => a.cmp(b),
            (ValueViewRef::Record(a), ValueViewRef::Record(b)) => {
                compare_spurs(a.type_tag, b.type_tag).then_with(|| a.fields.cmp(&b.fields))
            }
            (ValueViewRef::Bytevector(a), ValueViewRef::Bytevector(b)) => a.cmp(b),
            (ValueViewRef::I64Array(a), ValueViewRef::I64Array(b)) => a.cmp(b),
            (ValueViewRef::F64Array(a), ValueViewRef::F64Array(b)) => a
                .iter()
                .zip(b.iter())
                .map(|(x, y)| x.total_cmp(y))
                .find(|o| *o != std::cmp::Ordering::Equal)
                .unwrap_or_else(|| a.len().cmp(&b.len())),
            _ => type_order(self).cmp(&type_order(other)),
        }
    }
}

// ── Display ───────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    let mut iter = s.chars();
    let prefix: String = iter.by_ref().take(max).collect();
    if iter.next().is_none() {
        prefix
    } else {
        format!("{prefix}...")
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Grow the stack on demand so printing a deeply nested value can't
        // overflow the OS thread stack and abort the process.
        crate::stack::maybe_grow(|| match self.view_ref() {
            ValueViewRef::Nil => write!(f, "nil"),
            ValueViewRef::Bool(true) => write!(f, "#t"),
            ValueViewRef::Bool(false) => write!(f, "#f"),
            ValueViewRef::Int(n) => write!(f, "{n}"),
            ValueViewRef::BigInt(n) => write!(f, "{n}"),
            ValueViewRef::Rational(r) => write!(f, "{}/{}", r.numer(), r.denom()),
            ValueViewRef::Complex(c) => {
                write!(f, "{}", SemaNumber::Complex(Box::new((*c).clone())))
            }
            ValueViewRef::Float(n) => {
                if n.fract() == 0.0 {
                    write!(f, "{n:.1}")
                } else {
                    write!(f, "{n}")
                }
            }
            ValueViewRef::String(s) => {
                write!(f, "\"")?;
                for c in s.chars() {
                    match c {
                        '"' => write!(f, "\\\"")?,
                        '\\' => write!(f, "\\\\")?,
                        '\n' => write!(f, "\\n")?,
                        '\t' => write!(f, "\\t")?,
                        '\r' => write!(f, "\\r")?,
                        c => write!(f, "{c}")?,
                    }
                }
                write!(f, "\"")
            }
            ValueViewRef::Symbol(s) => with_resolved(s, |name| write!(f, "{name}")),
            ValueViewRef::Keyword(s) => with_resolved(s, |name| write!(f, ":{name}")),
            ValueViewRef::Char(c) => match c {
                ' ' => write!(f, "#\\space"),
                '\n' => write!(f, "#\\newline"),
                '\t' => write!(f, "#\\tab"),
                '\r' => write!(f, "#\\return"),
                '\0' => write!(f, "#\\nul"),
                _ => write!(f, "#\\{c}"),
            },
            ValueViewRef::List(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, ")")
            }
            ValueViewRef::Vector(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            ValueViewRef::Map(map) => {
                write!(f, "{{")?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{k} {v}")?;
                }
                write!(f, "}}")
            }
            ValueViewRef::HashMap(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by_key(|(k1, _)| *k1);
                write!(f, "{{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{k} {v}")?;
                }
                write!(f, "}}")
            }
            ValueViewRef::Lambda(l) => {
                if let Some(name) = &l.name {
                    with_resolved(*name, |n| write!(f, "<lambda {n}>"))
                } else {
                    write!(f, "<lambda>")
                }
            }
            ValueViewRef::Macro(m) => with_resolved(m.name, |n| write!(f, "<macro {n}>")),
            ValueViewRef::NativeFn(n) => write!(f, "<native-fn {}>", n.name),
            ValueViewRef::Prompt(p) => write!(f, "<prompt {} messages>", p.messages.len()),
            ValueViewRef::Message(m) => {
                write!(f, "<message {} \"{}\">", m.role, truncate(&m.content, 40))
            }
            ValueViewRef::Conversation(c) => {
                write!(f, "<conversation {} messages>", c.messages.len())
            }
            ValueViewRef::ToolDef(t) => write!(f, "<tool {}>", t.name),
            ValueViewRef::Agent(a) => write!(f, "<agent {}>", a.name),
            ValueViewRef::Thunk(t) => {
                if t.forced.borrow().is_some() {
                    write!(f, "<promise (forced)>")
                } else {
                    write!(f, "<promise>")
                }
            }
            ValueViewRef::Record(r) => {
                with_resolved(r.type_tag, |tag| write!(f, "#<record {tag}"))?;
                for field in &r.fields {
                    write!(f, " {field}")?;
                }
                write!(f, ">")
            }
            ValueViewRef::Bytevector(bv) => {
                write!(f, "#u8(")?;
                for (i, byte) in bv.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{byte}")?;
                }
                write!(f, ")")
            }
            ValueViewRef::F64Array(arr) => {
                write!(f, "#f64(")?;
                for (i, v) in arr.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, ")")
            }
            ValueViewRef::I64Array(arr) => {
                write!(f, "#i64(")?;
                for (i, v) in arr.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, ")")
            }
            ValueViewRef::MultiMethod(m) => {
                with_resolved(m.name, |n| write!(f, "<multimethod {n}>"))
            }
            ValueViewRef::Stream(s) => write!(f, "<stream:{}>", s.stream_type()),
            ValueViewRef::AsyncPromise(p) => match &*p.state.borrow() {
                PromiseState::Pending => write!(f, "<async-promise pending>"),
                PromiseState::Resolved(v) => write!(f, "<async-promise resolved: {v}>"),
                PromiseState::Rejected(e) => write!(f, "<async-promise rejected: {e}>"),
                PromiseState::Cancelled => write!(f, "<async-promise cancelled>"),
            },
            ValueViewRef::Channel(c) => {
                let len = c.buffer.borrow().len();
                if c.closed.get() {
                    write!(f, "<channel {len}/{} closed>", c.capacity)
                } else {
                    write!(f, "<channel {len}/{}>", c.capacity)
                }
            }
        })
    }
}

// ── Pretty-print ──────────────────────────────────────────────────

/// Pretty-print a value with line breaks and indentation when the compact
/// representation exceeds `max_width` columns.  Small values that fit in
/// one line are returned in the normal compact format.
pub fn pretty_print(value: &Value, max_width: usize) -> String {
    let mut buf = String::new();
    pp_value(value, 0, max_width, &mut buf);
    buf
}

/// Render `value` into `buf` at the given `indent` level.  If the compact
/// form fits in `max_width - indent` columns we use it; otherwise we break
/// the container across multiple lines.
fn pp_value(value: &Value, indent: usize, max_width: usize, buf: &mut String) {
    let compact = format!("{value}");
    let remaining = max_width.saturating_sub(indent);
    if compact.len() <= remaining {
        buf.push_str(&compact);
        return;
    }

    // Grow the stack on demand so pretty-printing a deeply nested value can't
    // overflow the OS thread stack and abort the process.
    crate::stack::maybe_grow(|| match value.view_ref() {
        ValueViewRef::List(items) => {
            pp_seq(items.iter(), '(', ')', indent, max_width, buf);
        }
        ValueViewRef::Vector(items) => {
            pp_seq(items.iter(), '[', ']', indent, max_width, buf);
        }
        ValueViewRef::Map(map) => {
            pp_map(
                map.iter().map(|(k, v)| (k.clone(), v.clone())),
                indent,
                max_width,
                buf,
            );
        }
        ValueViewRef::HashMap(map) => {
            let mut entries: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            entries.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
            pp_map(entries.into_iter(), indent, max_width, buf);
        }
        _ => buf.push_str(&compact),
    })
}

/// Pretty-print a list or vector.
fn pp_seq<'a>(
    items: impl Iterator<Item = &'a Value>,
    open: char,
    close: char,
    indent: usize,
    max_width: usize,
    buf: &mut String,
) {
    buf.push(open);
    let child_indent = indent + 1;
    let pad = " ".repeat(child_indent);
    for (i, item) in items.enumerate() {
        if i > 0 {
            buf.push('\n');
            buf.push_str(&pad);
        }
        pp_value(item, child_indent, max_width, buf);
    }
    buf.push(close);
}

/// Pretty-print a map (BTreeMap or HashMap).
fn pp_map(
    entries: impl Iterator<Item = (Value, Value)>,
    indent: usize,
    max_width: usize,
    buf: &mut String,
) {
    buf.push('{');
    let child_indent = indent + 1;
    let pad = " ".repeat(child_indent);
    for (i, (k, v)) in entries.enumerate() {
        if i > 0 {
            buf.push('\n');
            buf.push_str(&pad);
        }
        // Key is always compact
        let key_str = format!("{k}");
        buf.push_str(&key_str);

        // Check if the value fits inline after the key
        let inline_indent = child_indent + key_str.len() + 1;
        let compact_val = format!("{v}");
        let remaining = max_width.saturating_sub(inline_indent);

        if compact_val.len() <= remaining {
            // Fits inline
            buf.push(' ');
            buf.push_str(&compact_val);
        } else if is_compound(&v) {
            // Complex value: break to next line indented 2 from key
            let nested_indent = child_indent + 2;
            let nested_pad = " ".repeat(nested_indent);
            buf.push('\n');
            buf.push_str(&nested_pad);
            pp_value(&v, nested_indent, max_width, buf);
        } else {
            // Simple value that's just long: keep inline
            buf.push(' ');
            buf.push_str(&compact_val);
        }
    }
    buf.push('}');
}

/// Check whether a value is a compound container (list, vector, map, hashmap).
fn is_compound(value: &Value) -> bool {
    matches!(
        value.view(),
        ValueView::List(_) | ValueView::Vector(_) | ValueView::Map(_) | ValueView::HashMap(_)
    )
}

// ── Debug ─────────────────────────────────────────────────────────

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.view() {
            ValueView::Nil => write!(f, "Nil"),
            ValueView::Bool(b) => write!(f, "Bool({b})"),
            ValueView::Int(n) => write!(f, "Int({n})"),
            ValueView::BigInt(n) => write!(f, "Int({n})"),
            ValueView::Rational(r) => write!(f, "Rational({}/{})", r.numer(), r.denom()),
            ValueView::Complex(c) => write!(
                f,
                "Complex({})",
                SemaNumber::Complex(Box::new((*c).clone()))
            ),
            ValueView::Float(n) => write!(f, "Float({n})"),
            ValueView::String(s) => write!(f, "String({:?})", &**s),
            ValueView::Symbol(s) => write!(f, "Symbol({})", resolve(s)),
            ValueView::Keyword(s) => write!(f, "Keyword({})", resolve(s)),
            ValueView::Char(c) => write!(f, "Char({c:?})"),
            ValueView::List(items) => write!(f, "List({items:?})"),
            ValueView::Vector(items) => write!(f, "Vector({items:?})"),
            ValueView::Map(map) => write!(f, "Map({map:?})"),
            ValueView::HashMap(map) => write!(f, "HashMap({map:?})"),
            ValueView::Lambda(l) => write!(f, "{l:?}"),
            ValueView::Macro(m) => write!(f, "{m:?}"),
            ValueView::NativeFn(n) => write!(f, "{n:?}"),
            ValueView::Prompt(p) => write!(f, "{p:?}"),
            ValueView::Message(m) => write!(f, "{m:?}"),
            ValueView::Conversation(c) => write!(f, "{c:?}"),
            ValueView::ToolDef(t) => write!(f, "{t:?}"),
            ValueView::Agent(a) => write!(f, "{a:?}"),
            ValueView::Thunk(t) => write!(f, "{t:?}"),
            ValueView::Record(r) => write!(f, "{r:?}"),
            ValueView::Bytevector(bv) => write!(f, "Bytevector({bv:?})"),
            ValueView::F64Array(arr) => write!(f, "F64Array({arr:?})"),
            ValueView::I64Array(arr) => write!(f, "I64Array({arr:?})"),
            ValueView::MultiMethod(m) => write!(f, "{m:?}"),
            ValueView::Stream(s) => write!(f, "Stream({:?})", s.stream_type()),
            ValueView::AsyncPromise(p) => write!(f, "{p:?}"),
            ValueView::Channel(c) => write!(f, "{c:?}"),
        }
    }
}

// ── Env ───────────────────────────────────────────────────────────

/// A Sema environment: a chain of scopes with bindings.
#[derive(Debug, Clone)]
pub struct Env {
    pub bindings: Rc<RefCell<SpurMap<Spur, Value>>>,
    pub parent: Option<Rc<Env>>,
    pub version: Cell<u64>,
}

impl Env {
    pub fn new() -> Self {
        Env {
            bindings: Rc::new(RefCell::new(SpurMap::new())),
            parent: None,
            version: Cell::new(0),
        }
    }

    pub fn with_parent(parent: Rc<Env>) -> Self {
        Env {
            bindings: Rc::new(RefCell::new(SpurMap::new())),
            parent: Some(parent),
            version: Cell::new(0),
        }
    }

    /// Bump the environment's version counter. The VM's inline global cache is
    /// keyed on this version, so call this after mutating `bindings` through a
    /// different `Env` handle that shares the same `bindings` Rc but has its own
    /// version cell (e.g. a `load`ed module body run on a cloned-Env VM), so a
    /// VM observing this `Env` re-reads instead of serving a stale cached value.
    pub fn bump_version(&self) {
        self.version.set(self.version.get().wrapping_add(1));
    }

    pub fn get(&self, name: Spur) -> Option<Value> {
        if let Some(val) = self.bindings.borrow().get(&name) {
            Some(val.clone())
        } else if let Some(parent) = &self.parent {
            parent.get(name)
        } else {
            None
        }
    }

    pub fn get_str(&self, name: &str) -> Option<Value> {
        self.get(intern(name))
    }

    pub fn set(&self, name: Spur, val: Value) {
        self.bindings.borrow_mut().insert(name, val);
        self.bump_version();
    }

    pub fn set_str(&self, name: &str, val: Value) {
        self.set(intern(name), val);
    }

    /// Update a binding that already exists in the current scope.
    pub fn update(&self, name: Spur, val: Value) {
        let mut bindings = self.bindings.borrow_mut();
        if let Some(entry) = bindings.get_mut(&name) {
            *entry = val;
        } else {
            bindings.insert(name, val);
        }
        drop(bindings);
        self.bump_version();
    }

    /// Remove and return a binding from the current scope only.
    pub fn take(&self, name: Spur) -> Option<Value> {
        let result = self.bindings.borrow_mut().remove(&name);
        if result.is_some() {
            self.bump_version();
        }
        result
    }

    /// Remove and return a binding from any scope in the parent chain.
    pub fn take_anywhere(&self, name: Spur) -> Option<Value> {
        if let Some(val) = self.bindings.borrow_mut().remove(&name) {
            self.bump_version();
            Some(val)
        } else if let Some(parent) = &self.parent {
            parent.take_anywhere(name)
        } else {
            None
        }
    }

    /// Set a variable in the scope where it's defined (for set!).
    pub fn set_existing(&self, name: Spur, val: Value) -> bool {
        let mut bindings = self.bindings.borrow_mut();
        if let Some(entry) = bindings.get_mut(&name) {
            *entry = val;
            drop(bindings);
            self.bump_version();
            true
        } else {
            drop(bindings);
            if let Some(parent) = &self.parent {
                parent.set_existing(name, val)
            } else {
                false
            }
        }
    }

    /// Collect all bound variable names across all scopes (for suggestions).
    pub fn all_names(&self) -> Vec<Spur> {
        let mut names: Vec<Spur> = self.bindings.borrow().keys().copied().collect();
        if let Some(parent) = &self.parent {
            names.extend(parent.all_names());
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Iterate over bindings in the current scope only (not parent scopes).
    pub fn iter_bindings(&self, mut f: impl FnMut(Spur, &Value)) {
        let bindings = self.bindings.borrow();
        for (&spur, value) in bindings.iter() {
            f(spur, value);
        }
    }

    /// Get a binding from the current scope only (not parent scopes).
    pub fn get_local(&self, name: Spur) -> Option<Value> {
        self.bindings.borrow().get(&name).cloned()
    }

    /// Replace all bindings in the current scope with the given iterator.
    /// Used for bulk restore (e.g., undo/rollback).
    pub fn replace_bindings(&self, new_bindings: impl IntoIterator<Item = (Spur, Value)>) {
        let mut bindings = self.bindings.borrow_mut();
        bindings.clear();
        for (spur, value) in new_bindings {
            bindings.insert(spur, value);
        }
        drop(bindings);
        self.bump_version();
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;

    #[test]
    fn test_size_of_value() {
        assert_eq!(std::mem::size_of::<Value>(), 8);
    }

    #[test]
    fn test_spur_bits_round_trip() {
        // spur_to_bits / bits_to_spur must be exact inverses for freshly interned
        // keys, and a NaN-boxed symbol/keyword must round-trip to the same Spur.
        for s in ["x", "map", "string->symbol", "a-very-long-symbol-name", "λ"] {
            let spur = intern(s);
            assert_eq!(
                bits_to_spur(spur_to_bits(spur)),
                spur,
                "raw round-trip for {s:?}"
            );

            let sym = Value::symbol_from_spur(spur);
            assert_eq!(sym.as_symbol_spur(), Some(spur), "symbol Value for {s:?}");
            assert_eq!(resolve(spur), s);

            let kw = Value::keyword_from_spur(spur);
            assert_eq!(kw.as_keyword_spur(), Some(spur), "keyword Value for {s:?}");
        }
    }

    #[test]
    fn as_index_rejects_negative() {
        let e = Value::int(-1).as_index("test").unwrap_err();
        assert!(
            matches!(e.inner(), SemaError::Eval(_)),
            "expected Eval error, got {e:?}"
        );
        assert!(e.to_string().contains("test"));
    }

    #[test]
    fn as_index_accepts_non_negative() {
        assert_eq!(Value::int(0).as_index("test").unwrap(), 0);
        assert_eq!(Value::int(5).as_index("test").unwrap(), 5);
    }

    #[test]
    fn as_index_rejects_non_int() {
        assert!(Value::string("x").as_index("test").is_err());
    }

    #[test]
    fn test_nil() {
        let v = Value::nil();
        assert!(v.is_nil());
        assert!(!v.is_truthy());
        assert_eq!(v.type_name(), "nil");
        assert_eq!(format!("{v}"), "nil");
    }

    #[test]
    fn test_bool() {
        let t = Value::bool(true);
        let f = Value::bool(false);
        assert!(t.is_truthy());
        assert!(!f.is_truthy());
        assert_eq!(t.as_bool(), Some(true));
        assert_eq!(f.as_bool(), Some(false));
        assert_eq!(format!("{t}"), "#t");
        assert_eq!(format!("{f}"), "#f");
    }

    #[test]
    fn test_small_int() {
        let v = Value::int(42);
        assert_eq!(v.as_int(), Some(42));
        assert_eq!(v.type_name(), "int");
        assert_eq!(format!("{v}"), "42");

        let neg = Value::int(-100);
        assert_eq!(neg.as_int(), Some(-100));
        assert_eq!(format!("{neg}"), "-100");

        let zero = Value::int(0);
        assert_eq!(zero.as_int(), Some(0));
    }

    #[test]
    fn test_small_int_boundaries() {
        let max = Value::int(SMALL_INT_MAX);
        assert_eq!(max.as_int(), Some(SMALL_INT_MAX));

        let min = Value::int(SMALL_INT_MIN);
        assert_eq!(min.as_int(), Some(SMALL_INT_MIN));
    }

    #[test]
    fn test_big_int() {
        let big = Value::int(i64::MAX);
        assert_eq!(big.as_int(), Some(i64::MAX));
        assert_eq!(big.type_name(), "int");

        let big_neg = Value::int(i64::MIN);
        assert_eq!(big_neg.as_int(), Some(i64::MIN));

        // Just outside small range
        let just_over = Value::int(SMALL_INT_MAX + 1);
        assert_eq!(just_over.as_int(), Some(SMALL_INT_MAX + 1));
    }

    #[test]
    fn test_float() {
        let v = Value::float(3.14);
        assert_eq!(v.as_float(), Some(3.14));
        assert_eq!(v.type_name(), "float");

        let neg = Value::float(-0.5);
        assert_eq!(neg.as_float(), Some(-0.5));

        let inf = Value::float(f64::INFINITY);
        assert_eq!(inf.as_float(), Some(f64::INFINITY));

        let neg_inf = Value::float(f64::NEG_INFINITY);
        assert_eq!(neg_inf.as_float(), Some(f64::NEG_INFINITY));
    }

    #[test]
    fn test_float_nan() {
        let nan = Value::float(f64::NAN);
        let f = nan.as_float().unwrap();
        assert!(f.is_nan());
    }

    #[test]
    fn test_string() {
        let v = Value::string("hello");
        assert_eq!(v.as_str(), Some("hello"));
        assert_eq!(v.type_name(), "string");
        assert_eq!(format!("{v}"), "\"hello\"");
    }

    #[test]
    fn test_symbol() {
        let v = Value::symbol("foo");
        assert!(v.as_symbol_spur().is_some());
        assert_eq!(v.as_symbol(), Some("foo".to_string()));
        assert_eq!(v.type_name(), "symbol");
        assert_eq!(format!("{v}"), "foo");
    }

    #[test]
    fn test_keyword() {
        let v = Value::keyword("bar");
        assert!(v.as_keyword_spur().is_some());
        assert_eq!(v.as_keyword(), Some("bar".to_string()));
        assert_eq!(v.type_name(), "keyword");
        assert_eq!(format!("{v}"), ":bar");
    }

    #[test]
    fn test_char() {
        let v = Value::char('λ');
        assert_eq!(v.as_char(), Some('λ'));
        assert_eq!(v.type_name(), "char");
    }

    #[test]
    fn test_list() {
        let v = Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]);
        assert_eq!(v.as_list().unwrap().len(), 3);
        assert_eq!(v.type_name(), "list");
        assert_eq!(format!("{v}"), "(1 2 3)");
    }

    #[test]
    fn test_clone_immediate() {
        let v = Value::int(42);
        let v2 = v.clone();
        assert_eq!(v.as_int(), v2.as_int());
    }

    #[test]
    fn test_clone_heap() {
        let v = Value::string("hello");
        let v2 = v.clone();
        assert_eq!(v.as_str(), v2.as_str());
        // Both should work after clone
        assert_eq!(format!("{v}"), format!("{v2}"));
    }

    #[test]
    fn test_equality() {
        assert_eq!(Value::int(42), Value::int(42));
        assert_ne!(Value::int(42), Value::int(43));
        assert_eq!(Value::nil(), Value::nil());
        assert_eq!(Value::bool(true), Value::bool(true));
        assert_ne!(Value::bool(true), Value::bool(false));
        assert_eq!(Value::string("a"), Value::string("a"));
        assert_ne!(Value::string("a"), Value::string("b"));
        assert_eq!(Value::symbol("x"), Value::symbol("x"));
    }

    #[test]
    fn record_field_names_do_not_affect_language_semantics() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let a = Value::record(Record {
            type_tag: intern("point"),
            field_names: vec![intern("x"), intern("y")],
            fields: vec![Value::int(1), Value::int(2)],
        });
        let b = Value::record(Record {
            type_tag: intern("point"),
            field_names: vec![intern("left"), intern("top")],
            fields: vec![Value::int(1), Value::int(2)],
        });

        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
        assert_eq!(format!("{a}"), "#<record point 1 2>");
        assert_eq!(format!("{a}"), format!("{b}"));

        let mut a_hasher = DefaultHasher::new();
        a.hash(&mut a_hasher);
        let mut b_hasher = DefaultHasher::new();
        b.hash(&mut b_hasher);
        assert_eq!(a_hasher.finish(), b_hasher.finish());
    }

    #[test]
    fn test_big_int_equality() {
        assert_eq!(Value::int(i64::MAX), Value::int(i64::MAX));
        assert_ne!(Value::int(i64::MAX), Value::int(i64::MIN));
    }

    #[test]
    fn test_view_pattern_matching() {
        let v = Value::int(42);
        match v.view() {
            ValueView::Int(n) => assert_eq!(n, 42),
            _ => panic!("expected int"),
        }

        let v = Value::string("hello");
        match v.view() {
            ValueView::String(s) => assert_eq!(&**s, "hello"),
            _ => panic!("expected string"),
        }
    }

    #[test]
    fn test_env() {
        let env = Env::new();
        env.set_str("x", Value::int(42));
        assert_eq!(env.get_str("x"), Some(Value::int(42)));
    }

    #[test]
    fn test_native_fn_simple() {
        let f = NativeFn::simple("add1", |args| Ok(args[0].clone()));
        let ctx = EvalContext::new();
        assert!((f.func)(&ctx, &[Value::int(42)]).is_ok());
    }

    #[test]
    fn test_native_fn_with_ctx() {
        let f = NativeFn::with_ctx("get-depth", |ctx, _args| {
            Ok(Value::int(ctx.eval_depth.get() as i64))
        });
        let ctx = EvalContext::new();
        assert_eq!((f.func)(&ctx, &[]).unwrap(), Value::int(0));
    }

    #[test]
    fn test_drop_doesnt_leak() {
        // Create and drop many heap values to check for leaks
        for _ in 0..10000 {
            let _ = Value::string("test");
            let _ = Value::list(vec![Value::int(1), Value::int(2)]);
            let _ = Value::int(i64::MAX); // big int
        }
    }

    #[test]
    fn test_is_truthy() {
        assert!(!Value::nil().is_truthy());
        assert!(!Value::bool(false).is_truthy());
        assert!(Value::bool(true).is_truthy());
        assert!(Value::int(0).is_truthy());
        assert!(Value::int(1).is_truthy());
        assert!(Value::string("").is_truthy());
        assert!(Value::list(vec![]).is_truthy());
    }

    #[test]
    fn test_as_float_from_int() {
        assert_eq!(Value::int(42).as_float(), Some(42.0));
        assert_eq!(Value::float(3.14).as_float(), Some(3.14));
    }

    #[test]
    fn test_next_gensym_unique() {
        let a = next_gensym("x");
        let b = next_gensym("x");
        let c = next_gensym("y");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert!(a.starts_with("x__"));
        assert!(b.starts_with("x__"));
        assert!(c.starts_with("y__"));
    }

    #[test]
    fn test_next_gensym_counter_does_not_panic_near_max() {
        // Set counter near u64::MAX and verify no panic on wrapping
        GENSYM_COUNTER.with(|c| c.set(u64::MAX - 1));
        let a = next_gensym("z");
        assert!(a.contains(&(u64::MAX - 1).to_string()));
        // This would panic with `val + 1` instead of wrapping_add
        let b = next_gensym("z");
        assert!(b.contains(&u64::MAX.to_string()));
        // Wraps to 0
        let c = next_gensym("z");
        assert!(c.contains("__0"));
    }

    // ── StreamBox tests ──────────────────────────────────────────────

    #[derive(Debug)]
    struct TestStream {
        data: RefCell<Vec<u8>>,
        readable: bool,
        writable: bool,
    }

    impl TestStream {
        fn new(readable: bool, writable: bool) -> Self {
            TestStream {
                data: RefCell::new(Vec::new()),
                readable,
                writable,
            }
        }
    }

    impl SemaStream for TestStream {
        fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError> {
            let mut data = self.data.borrow_mut();
            let n = buf.len().min(data.len());
            buf[..n].copy_from_slice(&data[..n]);
            data.drain(..n);
            Ok(n)
        }

        fn write(&self, data: &[u8]) -> Result<usize, SemaError> {
            self.data.borrow_mut().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&self) -> Result<(), SemaError> {
            Ok(())
        }

        fn close(&self) -> Result<(), SemaError> {
            Ok(())
        }

        fn available(&self) -> Result<bool, SemaError> {
            Ok(!self.data.borrow().is_empty())
        }

        fn is_readable(&self) -> bool {
            self.readable
        }

        fn is_writable(&self) -> bool {
            self.writable
        }

        fn stream_type(&self) -> &'static str {
            "test"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn streambox_read_writes_data() {
        let sb = StreamBox::new(TestStream::new(true, true));
        sb.write(b"hello").unwrap();
        let mut buf = [0u8; 5];
        let n = sb.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn streambox_close_prevents_read() {
        let sb = StreamBox::new(TestStream::new(true, true));
        sb.close().unwrap();
        let mut buf = [0u8; 5];
        let err = sb.read(&mut buf).unwrap_err();
        assert!(err.to_string().contains("closed"));
    }

    #[test]
    fn streambox_close_prevents_write() {
        let sb = StreamBox::new(TestStream::new(true, true));
        sb.close().unwrap();
        let err = sb.write(b"data").unwrap_err();
        assert!(err.to_string().contains("closed"));
    }

    #[test]
    fn streambox_close_prevents_flush() {
        let sb = StreamBox::new(TestStream::new(true, true));
        sb.close().unwrap();
        let err = sb.flush().unwrap_err();
        assert!(err.to_string().contains("closed"));
    }

    #[test]
    fn streambox_double_close_is_noop() {
        let sb = StreamBox::new(TestStream::new(true, true));
        sb.close().unwrap();
        sb.close().unwrap(); // second close should be Ok
    }

    #[test]
    fn streambox_is_closed() {
        let sb = StreamBox::new(TestStream::new(true, true));
        assert!(!sb.is_closed());
        sb.close().unwrap();
        assert!(sb.is_closed());
    }

    #[test]
    fn streambox_is_readable() {
        let sb = StreamBox::new(TestStream::new(true, false));
        assert!(sb.is_readable());
        sb.close().unwrap();
        assert!(!sb.is_readable());
    }

    #[test]
    fn streambox_is_writable() {
        let sb = StreamBox::new(TestStream::new(false, true));
        assert!(sb.is_writable());
        sb.close().unwrap();
        assert!(!sb.is_writable());
    }

    #[test]
    fn streambox_available_when_closed() {
        let sb = StreamBox::new(TestStream::new(true, true));
        sb.close().unwrap();
        assert!(!sb.available().unwrap());
    }

    #[test]
    fn streambox_stream_type() {
        let sb = StreamBox::new(TestStream::new(true, true));
        assert_eq!(sb.stream_type(), "test");
    }

    #[test]
    fn bigint_roundtrip_and_normalize() {
        use num_bigint::BigInt;
        use std::str::FromStr;
        // A value beyond i64 stays a bignum and prints exactly.
        let big = BigInt::from_str("170141183460469231731687303715884105728").unwrap();
        let v = Value::from_bigint(big.clone());
        assert!(v.is_bigint());
        assert_eq!(v.to_string(), "170141183460469231731687303715884105728");
        assert_eq!(v.type_name(), "int");
        assert_eq!(v.as_int(), None); // does not fit i64
        assert_eq!(v.as_bigint(), Some(big));
        // A bignum that fits i64 normalizes back to a fixnum.
        let small = Value::from_bigint(BigInt::from(42));
        assert!(!small.is_bigint());
        assert_eq!(small.as_int(), Some(42));
        // Clone/Drop refcount safety.
        let v2 = v.clone();
        assert_eq!(v, v2);
    }

    #[test]
    fn rational_roundtrip_and_normalize() {
        use num_bigint::BigInt;
        use num_rational::BigRational;
        use num_traits::One;
        let third = Value::rational(BigRational::new(BigInt::one(), BigInt::from(3)));
        assert!(third.is_rational());
        assert_eq!(third.to_string(), "1/3");
        assert_eq!(third.type_name(), "rational");
        // 6/3 normalizes to the integer 2
        let two = Value::rational(BigRational::new(BigInt::from(6), BigInt::from(3)));
        assert!(!two.is_rational());
        assert_eq!(two.as_int(), Some(2));
        assert_eq!(third.clone(), third);
    }

    #[test]
    fn complex_roundtrip_and_normalize() {
        use crate::number::SemaNumber;
        let n = |v: i64| SemaNumber::from_i64(v);
        let c = Value::complex(n(3), n(4));
        assert!(c.is_complex());
        assert_eq!(c.to_string(), "3+4i");
        assert_eq!(c.type_name(), "complex");
        let comp = c.as_complex().unwrap();
        assert_eq!(comp.re, n(3));
        assert_eq!(comp.im, n(4));
        // Structural equality/clone/drop refcount safety.
        let c2 = c.clone();
        assert_eq!(c, c2);
        // Exact-zero imaginary part normalizes down to the real part alone.
        let real = Value::complex(n(5), n(0));
        assert!(!real.is_complex());
        assert_eq!(real.as_int(), Some(5));
    }
}
