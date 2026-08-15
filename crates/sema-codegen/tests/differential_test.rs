//! Randomized differential test: VM alone versus VM with the JIT installed.
//!
//! The hand-written cases in `equivalence_test.rs` check the cases the
//! implementation was designed around. This one checks the cases it was not:
//! it generates random expressions over the compilable subset, with operands
//! drawn from the values most likely to break a fast path — the fixnum
//! boundary, zero divisors, NaN, the infinities, and non-numbers — and demands
//! that both runs produce exactly the same result or exactly the same error.
//!
//! Everything is seeded, so a failure reproduces from the seed printed in the
//! assertion message.

use sema_eval::Interpreter;

/// Deterministic generator. A plain LCG: reproducibility matters here, quality
/// does not.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 16
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Operands chosen to sit on the edges the compiled fast paths care about.
const OPERANDS: &[&str] = &[
    "0",
    "1",
    "-1",
    "2",
    "-3",
    "7",
    "1000",
    // The fixnum range is [-2^44, 2^44 - 1]; anything past it must promote.
    "17592186044415",
    "-17592186044416",
    "17592186044414",
    "8796093022207",
    "-8796093022208",
    "0.0",
    "-0.0",
    "1.5",
    "-2.25",
    "1.0e30",
    "1.0e-30",
    "(/ 1.0 0.0)",
    "(/ -1.0 0.0)",
    "(/ 0.0 0.0)",
    "#t",
    "#f",
    "nil",
];

const BINARY_OPS: &[&str] = &["+", "-", "*", "/", "modulo", "<", ">", "<=", ">=", "="];
const UNARY_OPS: &[&str] = &["-", "not"];

/// Build a random expression over the parameters `a` and `b` plus literals.
fn expr(rng: &mut Rng, depth: u32) -> String {
    if depth == 0 || rng.below(4) == 0 {
        return match rng.below(4) {
            0 => "a".to_string(),
            1 => "b".to_string(),
            _ => rng.pick(OPERANDS).to_string(),
        };
    }
    match rng.below(6) {
        0 => format!("({} {})", rng.pick(UNARY_OPS), expr(rng, depth - 1)),
        1 => format!(
            "(if {} {} {})",
            expr(rng, depth - 1),
            expr(rng, depth - 1),
            expr(rng, depth - 1)
        ),
        2 => format!(
            "(let ((x {})) {})",
            expr(rng, depth - 1),
            expr(rng, depth - 1)
        ),
        3 => format!("(and {} {})", expr(rng, depth - 1), expr(rng, depth - 1)),
        4 => format!("(or {} {})", expr(rng, depth - 1), expr(rng, depth - 1)),
        _ => format!(
            "({} {} {})",
            rng.pick(BINARY_OPS),
            expr(rng, depth - 1),
            expr(rng, depth - 1)
        ),
    }
}

fn run(src: &str) -> String {
    let interp = Interpreter::new();
    match interp.eval_str(src) {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("error: {e}"),
    }
}

fn vm_only(src: &str) -> String {
    sema_codegen::uninstall();
    run(src)
}

fn with_jit(src: &str) -> String {
    sema_codegen::install().expect("no code generator for this host");
    sema_vm::jit::set_threshold(0);
    let out = run(src);
    sema_codegen::uninstall();
    out
}

/// Random arithmetic and control flow over two parameters.
#[test]
fn random_expressions_agree() {
    let mut rng = Rng(0x5E1A_0001);
    let mut compiled_any = false;

    for seed in 0..400u64 {
        let mut rng_case = Rng(rng.next() ^ seed);
        let body = expr(&mut rng_case, 3);
        let arg_a = OPERANDS[rng_case.below(OPERANDS.len())];
        let arg_b = OPERANDS[rng_case.below(OPERANDS.len())];
        let src = format!("(define (f a b) {body}) (f {arg_a} {arg_b})");

        let expected = vm_only(&src);

        sema_codegen::install().expect("no code generator for this host");
        sema_vm::jit::set_threshold(0);
        sema_vm::jit::reset_stats();
        let actual = run(&src);
        let stats = sema_vm::jit::stats();
        sema_codegen::uninstall();
        compiled_any |= stats.compiled > 0;

        assert_eq!(
            expected, actual,
            "seed {seed}: VM and JIT disagree on:\n{src}\n  vm  = {expected}\n  jit = {actual}"
        );
    }

    assert!(
        compiled_any,
        "no generated function was ever compiled — the test proved nothing"
    );
}

/// Random self-recursive loops. These exercise the parts the straight-line
/// cases cannot: the loop header, parameter rebinding, and the depth guard.
#[test]
fn random_loops_agree() {
    let mut rng = Rng(0x00C0_FFEE);

    for seed in 0..120u64 {
        let mut r = Rng(rng.next() ^ seed);
        let step = r.pick(&["+", "-", "*"]);
        let start = r.pick(&["0", "1", "-1", "3", "17592186044415"]);
        let limit = r.below(40) + 1;
        let src = format!(
            "(define (loop i acc) \
               (if (>= i {limit}) acc (loop (+ i 1) ({step} acc i)))) \
             (loop 0 {start})"
        );

        let expected = vm_only(&src);
        let actual = with_jit(&src);
        assert_eq!(
            expected, actual,
            "seed {seed}: VM and JIT disagree on:\n{src}\n  vm  = {expected}\n  jit = {actual}"
        );
    }
}

/// Random non-tail recursion, which runs on the machine stack in compiled code.
#[test]
fn random_recursion_agrees() {
    let mut rng = Rng(0x0BAD_C0DE);

    for seed in 0..60u64 {
        let mut r = Rng(rng.next() ^ seed);
        let combine = r.pick(&["+", "-", "*"]);
        let n = r.below(20);
        let src = format!("(define (rec n) (if (< n 1) 1 ({combine} n (rec (- n 1))))) (rec {n})");

        let expected = vm_only(&src);
        let actual = with_jit(&src);
        assert_eq!(
            expected, actual,
            "seed {seed}: VM and JIT disagree on:\n{src}\n  vm  = {expected}\n  jit = {actual}"
        );
    }
}
