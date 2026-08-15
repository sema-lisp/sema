//! Lowering of the compilable bytecode subset to Cranelift IR.
//!
//! Every runtime value is one `i64` holding NaN-boxed bits, so the operand
//! stack becomes a `Vec` of CLIF values and locals become CLIF variables. The
//! shape of an arithmetic op is always the same: test the tags, run the
//! hardware instruction on the unboxed operands, and branch to a shared bail
//! block whenever the fast path does not apply.
//!
//! Nothing here allocates and nothing here can raise. Every case that would
//! need either — fixnum overflow to bignum, inexact integer division, a
//! non-numeric operand, division by zero — bails, and the VM runs the call.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::BlockArg;
use cranelift_codegen::ir::{types, Block, InstBuilder, MemFlagsData, Value as ClifValue};
use cranelift_codegen::isa::TargetFrontendConfig;
use cranelift_frontend::{FunctionBuilder, Variable};
use std::collections::BTreeMap;

use sema_core::{
    Value, NAN_BOX_MASK, NAN_CANONICAL, NAN_INT_SMALL_PATTERN, NAN_PAYLOAD_BITS, NAN_PAYLOAD_MASK,
    NAN_TAG_MASK,
};
use sema_vm::jit::{JIT_BAIL, JIT_MAX_DEPTH};
use sema_vm::Op;

use crate::decode::{fixed_slot, Program};

/// Bits to shift by to sign-extend a 45-bit payload into an `i64` (64 - 45).
const SIGN_SHIFT: i64 = (64 - NAN_PAYLOAD_BITS) as i64;

/// Everything the translator needs from its caller.
pub struct TranslateCtx<'a> {
    pub program: &'a Program,
    /// Address of the backend's non-tail-recursion depth counter (a `u64`).
    pub depth_counter: *mut u64,
    /// Target layout, needed to finalize the builder.
    pub frontend_config: TargetFrontendConfig,
}

struct Translator<'a, 'b> {
    b: &'a mut FunctionBuilder<'b>,
    p: &'a Program,
    /// CLIF variable backing each local slot, indexed by slot number.
    locals: Vec<Variable>,
    /// Shared "guard failed" block: returns [`JIT_BAIL`].
    bail: Block,
    /// Block for each instruction pc that starts one.
    blocks: BTreeMap<u32, Block>,
    /// CLIF reference to this function, for self-calls.
    self_ref: Option<cranelift_codegen::ir::FuncRef>,
    depth_counter: *mut u64,
}

/// Translate `ctx.program` into the body of the function `b` is building.
///
/// `self_ref` must be present exactly when the program contains a non-tail
/// self-call; the caller declares the function in the module first so the body
/// can reference itself.
pub fn translate(
    mut b: FunctionBuilder<'_>,
    ctx: &TranslateCtx<'_>,
    self_ref: Option<cranelift_codegen::ir::FuncRef>,
) {
    let p = ctx.program;

    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);

    // Locals become CLIF variables: parameters take the incoming arguments, and
    // every other slot starts as nil exactly as a fresh VM frame would.
    let params: Vec<ClifValue> = b.block_params(entry).to_vec();
    let mut locals = Vec::with_capacity(p.n_locals as usize);
    for slot in 0..p.n_locals as usize {
        let var = b.declare_var(types::I64);
        let init = match params.get(slot) {
            Some(v) => *v,
            None => b.ins().iconst(types::I64, Value::NIL.raw_bits() as i64),
        };
        b.def_var(var, init);
        locals.push(var);
    }

    let bail = b.create_block();

    // One CLIF block per bytecode jump target, carrying the operand stack as
    // block parameters.
    let mut blocks = BTreeMap::new();
    for pc in &p.block_starts {
        let idx = p.index_of_pc[pc];
        let block = b.create_block();
        for _ in 0..p.depth_at[idx] {
            b.append_block_param(block, types::I64);
        }
        blocks.insert(*pc, block);
    }

    Translator {
        b: &mut b,
        p,
        locals,
        bail,
        blocks,
        self_ref,
        depth_counter: ctx.depth_counter,
    }
    .run();

    b.finalize(ctx.frontend_config);
}

impl Translator<'_, '_> {
    fn run(&mut self) {
        // The entry block is current and holds the incoming arguments. If pc 0
        // is itself a jump target (a self-tail-call loop header), fall into it;
        // otherwise keep emitting straight into the entry block.
        let mut stack: Vec<ClifValue> = Vec::new();
        let mut live = true;

        for idx in 0..self.p.instrs.len() {
            let instr = self.p.instrs[idx];
            if !self.p.reachable[idx] {
                continue;
            }
            if let Some(block) = self.blocks.get(&instr.pc).copied() {
                if live {
                    // Fall-through edge into a block other jumps also target:
                    // hand the operand stack over as block arguments.
                    let args = to_args(&stack);
                    self.b.ins().jump(block, &args);
                }
                self.b.switch_to_block(block);
                stack = self.b.block_params(block).to_vec();
                live = true;
            }
            debug_assert!(
                live,
                "reachable instruction at pc {} follows a terminator without \
                 starting a block",
                instr.pc
            );
            live = self.emit(instr, &mut stack);
        }

        // One shared exit that tells the VM to run this call itself.
        let bail = self.bail;
        self.b.switch_to_block(bail);
        let sentinel = self.b.ins().iconst(types::I64, JIT_BAIL as i64);
        self.b.ins().return_(&[sentinel]);

        self.b.seal_all_blocks();
    }

    /// Emit one instruction. Returns whether control falls through to the next.
    fn emit(&mut self, instr: crate::decode::Instr, stack: &mut Vec<ClifValue>) -> bool {
        use Op::*;
        match instr.op {
            Const => {
                let bits = self.p.consts[instr.arg as usize].raw_bits();
                let v = self.iconst(bits);
                stack.push(v);
            }
            Nil => {
                let v = self.iconst(Value::NIL.raw_bits());
                stack.push(v);
            }
            True => {
                let v = self.iconst(Value::TRUE.raw_bits());
                stack.push(v);
            }
            False => {
                let v = self.iconst(Value::FALSE.raw_bits());
                stack.push(v);
            }
            Pop => {
                stack.pop();
            }
            Dup => {
                let top = *stack.last().expect("Dup on empty operand stack");
                stack.push(top);
            }
            LoadLocal | LoadLocal0 | LoadLocal1 | LoadLocal2 | LoadLocal3 => {
                let slot = fixed_slot(instr.op).unwrap_or(instr.arg);
                let v = self.b.use_var(self.locals[slot as usize]);
                stack.push(v);
            }
            TakeLocal => {
                // Moving load: the slot reads as nil afterwards.
                let var = self.locals[instr.arg as usize];
                let v = self.b.use_var(var);
                let nil = self.iconst(Value::NIL.raw_bits());
                self.b.def_var(var, nil);
                stack.push(v);
            }
            StoreLocal | StoreLocal0 | StoreLocal1 | StoreLocal2 | StoreLocal3 => {
                let slot = fixed_slot(instr.op).unwrap_or(instr.arg);
                let v = stack.pop().expect("StoreLocal on empty operand stack");
                self.b.def_var(self.locals[slot as usize], v);
            }
            Jump => {
                let target = self.blocks[&instr.arg];
                let args = to_args(stack);
                self.b.ins().jump(target, &args);
                return false;
            }
            JumpIfFalse | JumpIfTrue => {
                let cond = stack.pop().expect("conditional jump on empty stack");
                let truthy = self.is_truthy(cond);
                let target = self.blocks[&instr.arg];
                // The taken edge hands the operand stack to the target block;
                // the fall-through edge goes to a fresh parameterless block, so
                // the same CLIF values simply stay live there.
                let taken = to_args(stack);
                let fallthrough = self.b.create_block();
                if instr.op == JumpIfTrue {
                    self.b.ins().brif(truthy, target, &taken, fallthrough, &[]);
                } else {
                    self.b.ins().brif(truthy, fallthrough, &[], target, &taken);
                }
                self.b.switch_to_block(fallthrough);
            }
            Return => {
                let v = stack.pop().expect("Return on empty operand stack");
                self.b.ins().return_(&[v]);
                return false;
            }
            Add | AddInt => self.binary_arith(stack, ArithOp::Add),
            Sub | SubInt => self.binary_arith(stack, ArithOp::Sub),
            Mul | MulInt => self.binary_arith(stack, ArithOp::Mul),
            Div => self.binary_arith(stack, ArithOp::Div),
            Mod => self.int_mod(stack),
            Eq | EqInt => self.compare(stack, CmpOp::Eq),
            Lt | LtInt => self.compare(stack, CmpOp::Lt),
            Gt => self.compare(stack, CmpOp::Gt),
            Le => self.compare(stack, CmpOp::Le),
            Ge => self.compare(stack, CmpOp::Ge),
            Negate => self.negate(stack),
            Not => {
                let v = stack.pop().expect("Not on empty operand stack");
                let truthy = self.is_truthy(v);
                let t = self.iconst(Value::TRUE.raw_bits());
                let f = self.iconst(Value::FALSE.raw_bits());
                // `not` of a truthy value is #f.
                let r = self.b.ins().select(truthy, f, t);
                stack.push(r);
            }
            CallSelf => {
                self.self_call(stack, instr.arg as usize);
            }
            SelfTailCall => {
                let argc = instr.arg as usize;
                let at = stack.len() - argc;
                let args: Vec<ClifValue> = stack.drain(at..).collect();
                // Rebind parameters, then reset the remaining slots to nil —
                // the VM's `self_tail_call` resizes the frame the same way.
                for (slot, arg) in args.iter().enumerate() {
                    self.b.def_var(self.locals[slot], *arg);
                }
                let nil = self.iconst(Value::NIL.raw_bits());
                for slot in argc..self.p.n_locals as usize {
                    self.b.def_var(self.locals[slot], nil);
                }
                let header = self.blocks[&0];
                self.b.ins().jump(header, &[]);
                return false;
            }
            other => unreachable!("translate reached non-whitelisted opcode {other:?}"),
        }
        true
    }

    // ── value-model helpers ───────────────────────────────────────

    fn iconst(&mut self, bits: u64) -> ClifValue {
        self.b.ins().iconst(types::I64, bits as i64)
    }

    /// `(v & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN`
    fn is_int(&mut self, v: ClifValue) -> ClifValue {
        let mask = self.iconst(NAN_TAG_MASK);
        let pattern = self.iconst(NAN_INT_SMALL_PATTERN);
        let tagged = self.b.ins().band(v, mask);
        self.b.ins().icmp(IntCC::Equal, tagged, pattern)
    }

    /// A value is a float exactly when it is not boxed.
    fn is_float(&mut self, v: ClifValue) -> ClifValue {
        let mask = self.iconst(NAN_BOX_MASK);
        let masked = self.b.ins().band(v, mask);
        self.b.ins().icmp(IntCC::NotEqual, masked, mask)
    }

    /// Sign-extend the 45-bit small-int payload to a full `i64`.
    fn unbox_int(&mut self, v: ClifValue) -> ClifValue {
        let payload_mask = self.iconst(NAN_PAYLOAD_MASK);
        let payload = self.b.ins().band(v, payload_mask);
        let up = self.b.ins().ishl_imm_s(payload, SIGN_SHIFT);
        self.b.ins().sshr_imm_s(up, SIGN_SHIFT)
    }

    /// Re-box an `i64`, branching to the bail block when it no longer fits the
    /// 45-bit fixnum range — the VM promotes those to a bignum, which compiled
    /// code must never allocate.
    fn box_int_checked(&mut self, x: ClifValue) -> ClifValue {
        let up = self.b.ins().ishl_imm_s(x, SIGN_SHIFT);
        let back = self.b.ins().sshr_imm_s(up, SIGN_SHIFT);
        let fits = self.b.ins().icmp(IntCC::Equal, back, x);
        let ok = self.b.create_block();
        self.b.ins().brif(fits, ok, &[], self.bail, &[]);
        self.b.switch_to_block(ok);

        let payload_mask = self.iconst(NAN_PAYLOAD_MASK);
        let payload = self.b.ins().band(x, payload_mask);
        let pattern = self.iconst(NAN_INT_SMALL_PATTERN);
        self.b.ins().bor(payload, pattern)
    }

    /// Reinterpret an `f64` as `Value` bits, mapping every NaN onto the
    /// canonical one.
    ///
    /// Hardware's default NaN is `0xFFF8_0000_0000_0000`, which is bit-identical
    /// to `Value::NIL`. Skipping this step would silently turn `(/ 0.0 0.0)`
    /// into nil.
    fn box_float(&mut self, f: ClifValue) -> ClifValue {
        let bits = self.b.ins().bitcast(types::I64, MemFlagsData::new(), f);
        let is_nan = self.b.ins().fcmp(FloatCC::NotEqual, f, f);
        let canonical = self.iconst(NAN_CANONICAL);
        self.b.ins().select(is_nan, canonical, bits)
    }

    /// Read a value as `f64`, whether it holds a fixnum or a float. Small ints
    /// are at most 45 bits, so the conversion is exact and mixed-mode
    /// comparisons stay faithful to the VM's exact `cmp_int_float`.
    fn as_float(&mut self, v: ClifValue, v_is_int: ClifValue) -> ClifValue {
        let as_int = self.unbox_int(v);
        let from_int = self.b.ins().fcvt_from_sint(types::F64, as_int);
        let from_bits = self.b.ins().bitcast(types::F64, MemFlagsData::new(), v);
        self.b.ins().select(v_is_int, from_int, from_bits)
    }

    /// Sema truthiness: everything except nil and `#f`.
    fn is_truthy(&mut self, v: ClifValue) -> ClifValue {
        let nil = self.iconst(Value::NIL.raw_bits());
        let f = self.iconst(Value::FALSE.raw_bits());
        let is_nil = self.b.ins().icmp(IntCC::Equal, v, nil);
        let is_false = self.b.ins().icmp(IntCC::Equal, v, f);
        let falsy = self.b.ins().bor(is_nil, is_false);
        let zero = self.b.ins().iconst(types::I8, 0);
        self.b.ins().icmp(IntCC::Equal, falsy, zero)
    }

    /// Branch to the bail block unless `cond` holds.
    fn guard(&mut self, cond: ClifValue) {
        let ok = self.b.create_block();
        self.b.ins().brif(cond, ok, &[], self.bail, &[]);
        self.b.switch_to_block(ok);
    }

    // ── arithmetic ────────────────────────────────────────────────

    /// Shared shape for `+ - * /`: an integer path, a float path for anything
    /// numeric that is not two fixnums, and a bail for everything else.
    fn binary_arith(&mut self, stack: &mut Vec<ClifValue>, op: ArithOp) {
        let b_val = stack.pop().expect("arith on empty operand stack");
        let a_val = stack.pop().expect("arith on empty operand stack");

        let a_is_int = self.is_int(a_val);
        let b_is_int = self.is_int(b_val);
        let both_int = self.b.ins().band(a_is_int, b_is_int);

        let int_block = self.b.create_block();
        let mixed_block = self.b.create_block();
        let merge = self.b.create_block();
        self.b.append_block_param(merge, types::I64);

        self.b
            .ins()
            .brif(both_int, int_block, &[], mixed_block, &[]);

        // Integer path.
        self.b.switch_to_block(int_block);
        let x = self.unbox_int(a_val);
        let y = self.unbox_int(b_val);
        let boxed = match op {
            ArithOp::Add => {
                // Both operands are at most 45 bits, so the i64 add cannot wrap;
                // only the fixnum range check can fail.
                let s = self.b.ins().iadd(x, y);
                self.box_int_checked(s)
            }
            ArithOp::Sub => {
                let s = self.b.ins().isub(x, y);
                self.box_int_checked(s)
            }
            ArithOp::Mul => {
                // A 45-bit by 45-bit product can exceed i64, so check the high
                // half before trusting the low one.
                let lo = self.b.ins().imul(x, y);
                let hi = self.b.ins().smulhi(x, y);
                let sign = self.b.ins().sshr_imm_s(lo, 63);
                let no_wrap = self.b.ins().icmp(IntCC::Equal, hi, sign);
                self.guard(no_wrap);
                self.box_int_checked(lo)
            }
            ArithOp::Div => {
                // Sema `/` is exact: an inexact quotient becomes a rational, and
                // a zero divisor raises. Both are the VM's job.
                let zero = self.b.ins().iconst(types::I64, 0);
                let nonzero = self.b.ins().icmp(IntCC::NotEqual, y, zero);
                self.guard(nonzero);
                let rem = self.b.ins().srem(x, y);
                let exact = self.b.ins().icmp(IntCC::Equal, rem, zero);
                self.guard(exact);
                let q = self.b.ins().sdiv(x, y);
                self.box_int_checked(q)
            }
        };
        self.b.ins().jump(merge, &[BlockArg::Value(boxed)]);

        // Not two fixnums: take the float path if both are numbers at all.
        self.b.switch_to_block(mixed_block);
        let a_is_float = self.is_float(a_val);
        let b_is_float = self.is_float(b_val);
        let a_num = self.b.ins().bor(a_is_int, a_is_float);
        let b_num = self.b.ins().bor(b_is_int, b_is_float);
        let both_num = self.b.ins().band(a_num, b_num);
        let float_block = self.b.create_block();
        self.b
            .ins()
            .brif(both_num, float_block, &[], self.bail, &[]);

        self.b.switch_to_block(float_block);
        let fa = self.as_float(a_val, a_is_int);
        let fb = self.as_float(b_val, b_is_int);
        let fr = match op {
            ArithOp::Add => self.b.ins().fadd(fa, fb),
            ArithOp::Sub => self.b.ins().fsub(fa, fb),
            ArithOp::Mul => self.b.ins().fmul(fa, fb),
            ArithOp::Div => self.b.ins().fdiv(fa, fb),
        };
        let boxed_f = self.box_float(fr);
        self.b.ins().jump(merge, &[BlockArg::Value(boxed_f)]);

        self.b.switch_to_block(merge);
        let result = self.b.block_params(merge)[0];
        stack.push(result);
    }

    /// `modulo` is floored, not truncated, and only the fixnum case is
    /// compiled: CLIF has no float remainder instruction, and bignums allocate.
    fn int_mod(&mut self, stack: &mut Vec<ClifValue>) {
        let b_val = stack.pop().expect("mod on empty operand stack");
        let a_val = stack.pop().expect("mod on empty operand stack");

        let a_is_int = self.is_int(a_val);
        let b_is_int = self.is_int(b_val);
        let both_int = self.b.ins().band(a_is_int, b_is_int);
        self.guard(both_int);

        let x = self.unbox_int(a_val);
        let y = self.unbox_int(b_val);
        let zero = self.b.ins().iconst(types::I64, 0);
        let nonzero = self.b.ins().icmp(IntCC::NotEqual, y, zero);
        self.guard(nonzero);

        // `srem` truncates toward zero; floored modulo differs whenever the
        // remainder is non-zero and its sign disagrees with the divisor.
        let rem = self.b.ins().srem(x, y);
        let rem_nonzero = self.b.ins().icmp(IntCC::NotEqual, rem, zero);
        let signs = self.b.ins().bxor(rem, y);
        let signs_differ = self.b.ins().icmp(IntCC::SignedLessThan, signs, zero);
        let needs_shift = self.b.ins().band(rem_nonzero, signs_differ);
        let shifted = self.b.ins().iadd(rem, y);
        let floored = self.b.ins().select(needs_shift, shifted, rem);

        // |result| < |y| <= fixnum max, so the range check always passes; it is
        // kept so every boxed integer goes through one checked path.
        let boxed = self.box_int_checked(floored);
        stack.push(boxed);
    }

    fn negate(&mut self, stack: &mut Vec<ClifValue>) {
        let v = stack.pop().expect("negate on empty operand stack");
        let v_is_int = self.is_int(v);

        let int_block = self.b.create_block();
        let float_check = self.b.create_block();
        let merge = self.b.create_block();
        self.b.append_block_param(merge, types::I64);
        self.b
            .ins()
            .brif(v_is_int, int_block, &[], float_check, &[]);

        self.b.switch_to_block(int_block);
        let x = self.unbox_int(v);
        let neg = self.b.ins().ineg(x);
        // Negating the smallest fixnum leaves the range, so this can bail.
        let boxed = self.box_int_checked(neg);
        self.b.ins().jump(merge, &[BlockArg::Value(boxed)]);

        self.b.switch_to_block(float_check);
        let v_is_float = self.is_float(v);
        let float_block = self.b.create_block();
        self.b
            .ins()
            .brif(v_is_float, float_block, &[], self.bail, &[]);
        self.b.switch_to_block(float_block);
        let f = self.b.ins().bitcast(types::F64, MemFlagsData::new(), v);
        let nf = self.b.ins().fneg(f);
        let boxed_f = self.box_float(nf);
        self.b.ins().jump(merge, &[BlockArg::Value(boxed_f)]);

        self.b.switch_to_block(merge);
        let result = self.b.block_params(merge)[0];
        stack.push(result);
    }

    /// Numeric comparison. Non-numeric operands bail: the VM falls back to
    /// string ordering and structural equality there.
    ///
    /// The four orderings are all built from one "less than", swapped and
    /// inverted exactly as the VM builds them (`Gt` is `lt(b, a)`, `Le` is
    /// `!lt(b, a)`, `Ge` is `!lt(a, b)`). That is not cosmetic: for a NaN
    /// operand `lt` is false, so the VM's `<=` and `>=` answer `#t` where the
    /// IEEE predicates answer `#f`. Compiled code has to give the same answer
    /// the VM gives.
    fn compare(&mut self, stack: &mut Vec<ClifValue>, op: CmpOp) {
        let b_val = stack.pop().expect("compare on empty operand stack");
        let a_val = stack.pop().expect("compare on empty operand stack");

        let (lhs, rhs) = match op {
            CmpOp::Eq | CmpOp::Lt | CmpOp::Ge => (a_val, b_val),
            CmpOp::Gt | CmpOp::Le => (b_val, a_val),
        };
        let invert = matches!(op, CmpOp::Le | CmpOp::Ge);

        let lhs_is_int = self.is_int(lhs);
        let rhs_is_int = self.is_int(rhs);
        let both_int = self.b.ins().band(lhs_is_int, rhs_is_int);

        let int_block = self.b.create_block();
        let mixed_block = self.b.create_block();
        let merge = self.b.create_block();
        self.b.append_block_param(merge, types::I64);
        self.b
            .ins()
            .brif(both_int, int_block, &[], mixed_block, &[]);

        self.b.switch_to_block(int_block);
        let x = self.unbox_int(lhs);
        let y = self.unbox_int(rhs);
        let icc = match op {
            CmpOp::Eq => IntCC::Equal,
            _ => IntCC::SignedLessThan,
        };
        let ic = self.b.ins().icmp(icc, x, y);
        let ib = self.bool_value(ic, invert);
        self.b.ins().jump(merge, &[BlockArg::Value(ib)]);

        self.b.switch_to_block(mixed_block);
        let lhs_is_float = self.is_float(lhs);
        let rhs_is_float = self.is_float(rhs);
        let lhs_num = self.b.ins().bor(lhs_is_int, lhs_is_float);
        let rhs_num = self.b.ins().bor(rhs_is_int, rhs_is_float);
        let both_num = self.b.ins().band(lhs_num, rhs_num);
        let float_block = self.b.create_block();
        let bail = self.bail;
        self.b.ins().brif(both_num, float_block, &[], bail, &[]);

        self.b.switch_to_block(float_block);
        let fa = self.as_float(lhs, lhs_is_int);
        let fb = self.as_float(rhs, rhs_is_int);
        let fcc = match op {
            CmpOp::Eq => FloatCC::Equal,
            _ => FloatCC::LessThan,
        };
        let fc = self.b.ins().fcmp(fcc, fa, fb);
        let fb_val = self.bool_value(fc, invert);
        self.b.ins().jump(merge, &[BlockArg::Value(fb_val)]);

        self.b.switch_to_block(merge);
        let result = self.b.block_params(merge)[0];
        stack.push(result);
    }

    /// Turn a CLIF condition into `#t` / `#f` bits, negating it when asked.
    fn bool_value(&mut self, cond: ClifValue, invert: bool) -> ClifValue {
        let t = self.iconst(Value::TRUE.raw_bits());
        let f = self.iconst(Value::FALSE.raw_bits());
        if invert {
            self.b.ins().select(cond, f, t)
        } else {
            self.b.ins().select(cond, t, f)
        }
    }

    // ── self-calls ────────────────────────────────────────────────

    /// A non-tail call to this same function.
    ///
    /// Compiled recursion grows the machine stack, which the VM's frame vector
    /// cannot see, so the depth is counted explicitly and bails at
    /// [`JIT_MAX_DEPTH`]. Re-running in the VM then produces the usual
    /// "stack overflow" error.
    fn self_call(&mut self, stack: &mut Vec<ClifValue>, argc: usize) {
        let at = stack.len() - argc;
        let args: Vec<ClifValue> = stack.drain(at..).collect();

        let counter = self.iconst(self.depth_counter as u64);
        let flags = MemFlagsData::trusted();
        let depth = self.b.ins().load(types::I64, flags, counter, 0);
        let limit = self.iconst(JIT_MAX_DEPTH);
        let under = self.b.ins().icmp(IntCC::UnsignedLessThan, depth, limit);
        self.guard(under);

        let deeper = self.b.ins().iadd_imm_s(depth, 1);
        self.b.ins().store(flags, deeper, counter, 0);

        let self_ref = self.self_ref.expect("self-call without a self reference");
        let call = self.b.ins().call(self_ref, &args);
        let result = self.b.inst_results(call)[0];

        // Restore the depth on both the success and the bail path, so an
        // abandoned call leaves the counter exactly as it found it.
        self.b.ins().store(flags, depth, counter, 0);

        let sentinel = self.iconst(JIT_BAIL);
        let ok = self.b.ins().icmp(IntCC::NotEqual, result, sentinel);
        self.guard(ok);

        stack.push(result);
    }
}

/// Cranelift block arguments for the current operand stack.
fn to_args(stack: &[ClifValue]) -> Vec<BlockArg> {
    stack.iter().map(|v| BlockArg::Value(*v)).collect()
}

#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Copy)]
enum CmpOp {
    Eq,
    Lt,
    Gt,
    Le,
    Ge,
}
