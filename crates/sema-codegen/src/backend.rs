//! The Cranelift JIT backend.
//!
//! Owns the Cranelift module, the emitted code memory, and the recursion-depth
//! counter that generated code reads. One backend is installed per thread; its
//! code stays mapped for as long as it lives, which is what
//! `sema_vm::jit::JitBackend`'s contract requires.

use std::cell::RefCell;
use std::rc::Rc;

use cranelift_codegen::ir::{types, AbiParam};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};

use sema_vm::jit::JitBackend;
use sema_vm::Function;

use crate::decode::{analyze, Reject};
use crate::translate::{translate, TranslateCtx};

/// A Cranelift-backed native code generator.
pub struct CraneliftBackend {
    inner: RefCell<Inner>,
    /// Non-tail self-recursion depth, read and written by generated code.
    ///
    /// Boxed so its address is stable and can be baked into the emitted code,
    /// and owned per backend so it is never shared across threads — the whole
    /// backend, like everything else in the runtime, is single-threaded.
    depth: Box<u64>,
    /// Why each rejected function was rejected. Diagnostic only.
    rejections: RefCell<Vec<(String, Reject)>>,
}

struct Inner {
    module: JITModule,
    ctx: cranelift_codegen::Context,
    builder_ctx: FunctionBuilderContext,
    /// Serial number for generated symbol names.
    next_id: u32,
}

/// Reasons a backend could not be created on this host.
#[derive(Debug)]
pub enum BackendError {
    /// Cranelift has no register-allocation/encoding support for this target.
    UnsupportedTarget(String),
    /// A Cranelift setting was rejected.
    Settings(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::UnsupportedTarget(m) => {
                write!(f, "no native code generator for this target: {m}")
            }
            BackendError::Settings(m) => write!(f, "code generator settings rejected: {m}"),
        }
    }
}

impl std::error::Error for BackendError {}

impl CraneliftBackend {
    /// Build a backend for the host machine.
    pub fn new() -> Result<Self, BackendError> {
        // The host ISA is resolved explicitly rather than through
        // `JITBuilder::with_flags`, which panics when the host is unsupported.
        // A host Cranelift cannot target is a normal outcome here: the caller
        // keeps running on the VM.
        let isa_builder = cranelift_native::builder()
            .map_err(|msg| BackendError::UnsupportedTarget(msg.to_string()))?;

        let mut flags = settings::builder();
        // `speed` rather than `speed_and_size`: compiled bodies are small and
        // hot, and total compile time is dominated by module bookkeeping anyway.
        let mut set = |name: &str, value: &str| -> Result<(), BackendError> {
            flags
                .set(name, value)
                .map_err(|e| BackendError::Settings(format!("{name}={value}: {e}")))
        };
        set("opt_level", "speed")?;
        // Both are what `cranelift-jit` itself requires: code is emitted at a
        // fixed address it chooses, and libcalls must use long-range relocations
        // that can reach anywhere in the process.
        set("use_colocated_libcalls", "false")?;
        set("is_pic", "false")?;

        let isa = isa_builder
            .finish(settings::Flags::new(flags))
            .map_err(|e| BackendError::Settings(e.to_string()))?;

        let module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
        Ok(CraneliftBackend {
            inner: RefCell::new(Inner {
                ctx: module.make_context(),
                module,
                builder_ctx: FunctionBuilderContext::new(),
                next_id: 0,
            }),
            depth: Box::new(0),
            rejections: RefCell::new(Vec::new()),
        })
    }

    /// Build a backend and install it on the current thread.
    pub fn install() -> Result<(), BackendError> {
        let backend = Rc::new(CraneliftBackend::new()?);
        sema_vm::jit::threshold_from_env();
        sema_vm::jit::install_backend(backend);
        Ok(())
    }

    /// Rejected functions and their reasons, oldest first.
    pub fn rejections(&self) -> Vec<(String, Reject)> {
        self.rejections.borrow().clone()
    }

    /// Current non-tail self-recursion depth. Always zero between calls; a
    /// non-zero reading means generated code is on the stack right now.
    pub fn depth(&self) -> u64 {
        *self.depth
    }

    fn compile_checked(&self, func: &Rc<Function>) -> Result<*const u8, Reject> {
        let program = analyze(func)?;

        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;

        let mut sig = inner.module.make_signature();
        for _ in 0..program.arity {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.returns.push(AbiParam::new(types::I64));

        let name = format!("sema_jit_{}", inner.next_id);
        inner.next_id += 1;

        // Declared before the body is built so the body can call itself.
        let func_id: FuncId = inner
            .module
            .declare_function(&name, Linkage::Export, &sig)
            .expect("declaring a fresh JIT symbol cannot collide");

        inner.module.clear_context(&mut inner.ctx);
        inner.ctx.func.signature = sig;

        let needs_self = program.has_self_call;
        let frontend_config = inner.module.target_config();
        {
            let builder = FunctionBuilder::new(&mut inner.ctx.func, &mut inner.builder_ctx);
            let self_ref = if needs_self {
                Some(inner.module.declare_func_in_func(func_id, builder.func))
            } else {
                None
            };
            let ctx = TranslateCtx {
                program: &program,
                depth_counter: (&*self.depth) as *const u64 as *mut u64,
                frontend_config,
            };
            translate(builder, &ctx, self_ref);
        }

        inner
            .module
            .define_function(func_id, &mut inner.ctx)
            .expect("Cranelift rejected generated IR");
        inner.module.clear_context(&mut inner.ctx);
        inner
            .module
            .finalize_definitions()
            .expect("finalizing JIT code failed");

        Ok(inner.module.get_finalized_function(func_id))
    }
}

impl JitBackend for CraneliftBackend {
    fn compile(&self, func: &Rc<Function>) -> Option<*const u8> {
        match self.compile_checked(func) {
            Ok(ptr) => Some(ptr),
            Err(reason) => {
                let name = func
                    .name
                    .map(sema_core::resolve)
                    .unwrap_or_else(|| "<lambda>".to_string());
                self.rejections.borrow_mut().push((name, reason));
                None
            }
        }
    }

    fn name(&self) -> &'static str {
        "cranelift"
    }
}
