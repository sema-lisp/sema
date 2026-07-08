# Full Numeric Tower Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Sema a complete R7RS-style numeric tower — arbitrary-precision integers (bignums), exact rationals, inexact reals, and complex numbers — with correct exactness contagion, so `(factorial 100)`, `(/ 1 3)`, and `(sqrt -1)` all produce mathematically correct values instead of overflowing, truncating, or erroring.

**Architecture:** The tower math is built and unit-tested in isolation as a standalone `SemaNumber` type in `sema-core/src/number.rs` (Phase 0), depending only on `num-bigint`/`num-rational`. It is then wired into the NaN-boxed `Value` (Phases 1–3) via three new heap tags (`TAG_BIGINT`, `TAG_RATIONAL`, `TAG_COMPLEX`), each a **leaf** heap type (holds no `Value`, so the cycle collector treats it exactly like the existing big-int/string leaves). Arithmetic lifts operands into `SemaNumber`, computes in the tower, and lowers the result back to the tightest `Value` representation. The VM's inline `i64`/`f64` fast paths are preserved; they fall through to the tower only on overflow or non-fixnum operands.

**Tech Stack:** Rust 2021, `num-bigint`, `num-rational`, `num-integer`, `num-traits` (pure-Rust, no C deps, single-threaded-friendly). NaN-boxed `Value(u64)`. Bytecode VM (`sema-vm`) as the sole evaluator.

## Global Constraints

- **Rust 2021, single-threaded** — use `Rc`, never `Arc`. Numeric heap payloads are `Rc<BigInt>` / `Rc<BigRational>` / `Rc<Complex>`.
- **Errors** — construct with `SemaError::eval()` / `::type_error()` / `::arity()`, never raw enum variants. Add `.with_hint(...)` for user-actionable guidance.
- **Sema naming (Decision #24)** — new builtins are slash-namespaced only if they belong to a namespace; the R7RS numeric names are canonical and unprefixed (`exact?`, `rational?`, `numerator`, `make-rectangular`, `exact->inexact`). Predicates end in `?`.
- **Dual arithmetic must stay in lockstep** — every semantic change to arithmetic/comparison exists in BOTH `sema-stdlib` (first-class `+ - * / < = …`) and `sema-vm` (`vm_add/vm_sub/vm_mul/vm_div/vm_eq/vm_lt` + the inline `ADD_INT`/`SUB_INT`/`MUL_INT`/`LT_INT` opcodes). A change to one without the other is a bug the `eval_tests!` oracle will catch only if a test covers it — so every such task adds a test that exercises both paths.
- **`eval_tests!` literal is the oracle** — there is no second backend. Each test pins `$input => $expected`; the expected literal is ground truth. Tests live in `crates/sema/tests/eval_test.rs`.
- **Bytecode format is versioned** — any new serialized constant kind requires updating `crates/sema-vm/src/serialize.rs` AND the spec at `website/docs/internals/bytecode-format.md`. Verify with `jake smoke-bytecode`.
- **NaN-box tag budget** — tags are 6 bits (0–63); 30 are used (`TAG_NIL`..`TAG_CHANNEL` = 0..29). This plan appends tags 30, 31, 32. Never renumber an existing tag (they index `.semac` files via serialize, and the runtime `view()` dispatch).
- **Build/test commands** — `jake build` (dev), `jake test` (all), `jake lint` (fmt-check + clippy -D warnings), `cargo test -p <crate>` (one crate), `cargo test -p sema --test eval_test -- <name>` (one eval test). Run `jake lint` before every commit.
- **GC leaf invariant** — `BigInt`, `BigRational`, and `Complex` hold no `Value`, so they are **leaves**: they get `Clone`/`Drop`/`heap_strong_count`/`view` arms but are NEVER registered as GC candidates and fall into the `_ => true` arm of `trace_value` (like strings and big-ints today). Do not add them to `cycle.rs`.

## Design Decisions (locked)

1. **Backend crates:** `num-bigint`, `num-rational`, `num-integer`, `num-traits`. NOT `num-complex` (a complex with independently-exact-or-inexact components does not fit `Complex<T>`; we define our own two-component `Complex`). NOT `rug`/`malachite` (C deps / heavier).
2. **Integer representation stays layered for speed:** `i64` values keep the existing inline `TAG_INT_SMALL` (45-bit immediate) and `TAG_INT_BIG` (`Rc<i64>`, for i64 values that exceed 45 bits). Integers **outside i64 range** use the new `TAG_BIGINT` (`Rc<BigInt>`). So `Value::as_int() -> Option<i64>` returns `None` for a bignum by design (it is "as i64"); a new `Value::as_bigint()` lifts any integer to `BigInt`.
3. **Canonical normalization (the tower's core invariant), applied by every lowering constructor:**
   - A `BigRational` with denominator 1 → integer path (§2's tightest integer form).
   - A `BigInt` in `i64` range → `Value::int` (fixnum/int-big), never `TAG_BIGINT`.
   - A `Complex` whose imaginary part is an **exact zero** → its real part (a real number), never `TAG_COMPLEX`.
   - Exactness contagion: any inexact (float) operand makes the whole result inexact.
4. **`SemaNumber` internal tower type** (in `number.rs`) is the arithmetic currency:
   ```rust
   pub enum SemaNumber {
       Integer(BigInt),        // exact integer, any magnitude
       Rational(BigRational),  // exact, denom > 1, reduced (num-rational guarantees this)
       Real(f64),              // inexact real
       Complex(Box<Complex>),  // non-real; components are never Complex
   }
   pub struct Complex { pub re: SemaNumber, pub im: SemaNumber } // re/im ∈ {Integer,Rational,Real}
   ```
5. **View enums gain three variants** — `ValueView`/`ValueViewRef` get `BigInt`, `Rational`, `Complex`. `Int(i64)` remains for fixnum/int-big. The compiler's exhaustiveness checking then drives every match site that must handle the new kinds; sites with a `_` arm compile unchanged and are audited per-phase.
6. **Reader literal forms:** `1/3` (rational), `3+4i` / `-2i` / `+i` (complex), `#x1F`/`#o17`/`#b101`/`#d10` (radix), `#e`/`#i` (exactness). Added incrementally, each with round-trip fuzzer coverage.

---

## File Structure

**New files:**
- `crates/sema-core/src/number.rs` — the standalone `SemaNumber`/`Complex` tower: arithmetic, comparison, exactness, parsing-from-parts, formatting. Zero dependency on NaN-boxing. This is the mathematical core, unit-tested in isolation.
- `crates/sema-docs/entries/exact.md`, `rational.md`, `numerator.md`, `make-rectangular.md`, … — one markdown doc per new builtin (Phase 6).

**Modified files (by area):**
- `crates/sema-core/Cargo.toml` — add the four `num-*` deps.
- `crates/sema-core/src/lib.rs` — `pub mod number;` + re-exports.
- `crates/sema-core/src/value.rs` — new tags, constructors, `ValueView`/`ValueViewRef` variants, and every trait/dispatch site (`view`, `view_ref`, `heap_ptr`, `heap_strong_count`, `type_name`, `is_int`, `Clone`, `Drop`, `PartialEq`, `Hash`, `Ord`, `Display`, `Debug`, `is_compound`), plus the `as_number`/`from_number`/`as_bigint` bridge.
- `crates/sema-core/src/cycle.rs` — **no logic change**; the three new leaf tags fall through `trace_value`'s `_` arm. (A one-line doc-comment update listing them as leaves.)
- `crates/sema-vm/src/serialize.rs` — `VAL_BIGINT`/`VAL_RATIONAL`/`VAL_COMPLEX` constant kinds in `serialize_value`/`deserialize_value`.
- `crates/sema-vm/src/vm.rs` — `vm_add/sub/mul/div/eq/lt` tower fallthrough; inline `*_INT` opcodes promote on overflow.
- `crates/sema-stdlib/src/arithmetic.rs` — `+ - * / mod` over the tower.
- `crates/sema-stdlib/src/comparison.rs` — `< > <= >= = zero? positive? negative? even? odd?` over the tower.
- `crates/sema-stdlib/src/predicates.rs` — `number? integer? float?` fixed + new `rational? real? complex? exact? inexact? exact-integer? nan? infinite?`.
- `crates/sema-stdlib/src/math.rs` — every math fn generalized; new `numerator denominator real-part imag-part magnitude angle make-rectangular make-polar exact->inexact inexact->exact exact-integer-sqrt rationalize number->string string->number`.
- `crates/sema-stdlib/src/bitwise.rs` — bignum-aware bit ops.
- `crates/sema-reader/src/lexer.rs` — `read_number` extended for rationals, complex, radix, exactness prefixes; new `Token` variants.
- `crates/sema-reader/src/reader.rs` — consume the new tokens into `Value`s.
- `crates/sema-core/src/json.rs`, `crates/sema-stdlib/src/json.rs` — encode/decode bignum/rational/complex.
- `fuzz/grammar-fuzz.sema` — emit and round-trip the new literal forms.
- `docs/limitations.md` — remove the "No Full Numeric Tower" entry.
- `website/docs/internals/bytecode-format.md` — document the new constant kinds.

---

## Phase 0 — Standalone `SemaNumber` tower (no NaN-boxing)

Build and fully unit-test the mathematics in isolation. Nothing here touches `Value`. Every task is a `cargo test -p sema-core number::` cycle. This de-risks all later phases: the tower math is proven before it is wired into the runtime.

### Task 0.1: Add the numeric backend dependencies

**Files:**
- Modify: `crates/sema-core/Cargo.toml`

**Interfaces:**
- Produces: the `num_bigint::BigInt`, `num_rational::BigRational`, `num_integer`, `num_traits` crates available to `sema-core`.

- [ ] **Step 1: Add the dependencies**

In `crates/sema-core/Cargo.toml`, under `[dependencies]`, add (keep the existing entries; match the alphabetical style already present):

```toml
num-bigint = "0.4"
num-integer = "0.1"
num-rational = "0.4"
num-traits = "0.2"
```

- [ ] **Step 2: Verify they resolve and compile**

Run: `cargo build -p sema-core`
Expected: builds clean (downloads the crates, no code using them yet).

- [ ] **Step 3: Commit**

```bash
git add crates/sema-core/Cargo.toml Cargo.lock
git commit -m "deps: add num-bigint/num-rational/num-integer/num-traits to sema-core"
```

### Task 0.2: Create the `number` module skeleton with the type definitions

**Files:**
- Create: `crates/sema-core/src/number.rs`
- Modify: `crates/sema-core/src/lib.rs`

**Interfaces:**
- Produces: `sema_core::number::{SemaNumber, Complex}` (the enum from Decision #4), plus `SemaNumber::is_exact()`, `SemaNumber::is_integer()`, `SemaNumber::is_real()`.

- [ ] **Step 1: Write the failing test**

Create `crates/sema-core/src/number.rs` with the type definitions and one classification test:

```rust
//! The Sema numeric tower: exact integers (arbitrary precision), exact
//! rationals, inexact reals, and complex numbers. This module is the
//! arithmetic currency — `Value` lifts operands into `SemaNumber`, computes
//! here, and lowers the result back to the tightest `Value` representation.
//! It has NO dependency on NaN-boxing and is unit-tested in isolation.

use num_bigint::BigInt;
use num_rational::BigRational;

/// A number anywhere in the tower. Invariants (upheld by every constructor
/// and arithmetic op via `normalize`):
/// - `Rational` is reduced and its denominator is > 1 (denom == 1 ⇒ `Integer`).
/// - `Complex`'s imaginary part is never an exact zero (⇒ the real part alone).
/// - `Complex` components are themselves never `Complex`.
#[derive(Clone, Debug)]
pub enum SemaNumber {
    Integer(BigInt),
    Rational(BigRational),
    Real(f64),
    Complex(Box<Complex>),
}

/// A non-real number `re + im·i`. Components are `Integer`, `Rational`, or
/// `Real` — never `Complex`. Exactness is per-component (a complex is exact
/// iff both components are exact).
#[derive(Clone, Debug)]
pub struct Complex {
    pub re: SemaNumber,
    pub im: SemaNumber,
}

impl SemaNumber {
    /// True unless any component is an inexact `Real`.
    pub fn is_exact(&self) -> bool {
        match self {
            SemaNumber::Integer(_) | SemaNumber::Rational(_) => true,
            SemaNumber::Real(_) => false,
            SemaNumber::Complex(c) => c.re.is_exact() && c.im.is_exact(),
        }
    }

    /// True for `Integer` and for any real-valued number equal to an integer.
    /// (A `Real` like `2.0` is an integer in the R7RS `integer?` sense.)
    pub fn is_integer(&self) -> bool {
        match self {
            SemaNumber::Integer(_) => true,
            SemaNumber::Rational(_) => false,
            SemaNumber::Real(f) => f.is_finite() && f.fract() == 0.0,
            SemaNumber::Complex(_) => false,
        }
    }

    /// True for everything except `Complex`.
    pub fn is_real(&self) -> bool {
        !matches!(self, SemaNumber::Complex(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_traits::One;

    #[test]
    fn classification() {
        assert!(SemaNumber::Integer(BigInt::from(5)).is_exact());
        assert!(SemaNumber::Integer(BigInt::from(5)).is_integer());
        assert!(!SemaNumber::Real(2.5).is_exact());
        assert!(SemaNumber::Real(2.0).is_integer());
        assert!(!SemaNumber::Real(2.5).is_integer());
        let half = SemaNumber::Rational(BigRational::new(BigInt::one(), BigInt::from(2)));
        assert!(half.is_exact());
        assert!(!half.is_integer());
        assert!(half.is_real());
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/sema-core/src/lib.rs`, add `pub mod number;` alongside the other `pub mod` declarations (near `pub mod num;` if present — keep them adjacent).

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p sema-core number::tests::classification`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs crates/sema-core/src/lib.rs
git commit -m "core: add SemaNumber tower type skeleton with classification"
```

### Task 0.3: Normalization — collapse to the tightest exact form

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`, `Complex`.
- Produces: `SemaNumber::normalize(self) -> SemaNumber` — a `Rational` with denom 1 becomes `Integer`; a `Complex` with exact-zero imaginary becomes its real part; recurses into complex components.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `number.rs`:

```rust
#[test]
fn normalize_collapses() {
    use num_traits::Zero;
    // 4/2 → Integer(2)
    let r = SemaNumber::Rational(BigRational::new(BigInt::from(4), BigInt::from(2)));
    assert!(matches!(r.normalize(), SemaNumber::Integer(n) if n == BigInt::from(2)));
    // 3 + 0i → Integer(3)
    let c = SemaNumber::Complex(Box::new(Complex {
        re: SemaNumber::Integer(BigInt::from(3)),
        im: SemaNumber::Integer(BigInt::zero()),
    }));
    assert!(matches!(c.normalize(), SemaNumber::Integer(n) if n == BigInt::from(3)));
    // 3 + 0.0i stays complex (0.0 is an INEXACT zero, not exact zero)
    let c2 = SemaNumber::Complex(Box::new(Complex {
        re: SemaNumber::Integer(BigInt::from(3)),
        im: SemaNumber::Real(0.0),
    }));
    assert!(matches!(c2.normalize(), SemaNumber::Complex(_)));
}
```

- [ ] **Step 2: Implement `normalize`**

Add to `impl SemaNumber` in `number.rs`:

```rust
use num_traits::{One, Zero};

impl SemaNumber {
    /// Collapse to the tightest canonical form (see the type invariants).
    /// Cheap and idempotent; every lowering constructor and arithmetic result
    /// passes through it.
    pub fn normalize(self) -> SemaNumber {
        match self {
            SemaNumber::Rational(r) => {
                if r.denom().is_one() {
                    SemaNumber::Integer(r.numer().clone())
                } else {
                    SemaNumber::Rational(r)
                }
            }
            SemaNumber::Complex(c) => {
                let re = c.re.normalize();
                let im = c.im.normalize();
                // Exact zero imaginary part ⇒ a real number. An inexact 0.0
                // must be preserved (the value is still non-real per R7RS).
                let im_is_exact_zero = matches!(&im, SemaNumber::Integer(n) if n.is_zero());
                if im_is_exact_zero {
                    re
                } else {
                    SemaNumber::Complex(Box::new(Complex { re, im }))
                }
            }
            other => other,
        }
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p sema-core number::tests::normalize_collapses`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber::normalize collapses to tightest exact form"
```

### Task 0.4: Coercion — lift two operands to a common tower level

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`.
- Produces: `SemaNumber::to_f64(&self) -> f64` (lossy real projection; complex → NaN); a private `fn promote(a: SemaNumber, b: SemaNumber) -> (SemaNumber, SemaNumber)` that lifts both to the same level (Integer < Rational < Real < Complex), used by every binary op.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
#[test]
fn to_f64_projection() {
    assert_eq!(SemaNumber::Integer(BigInt::from(7)).to_f64(), 7.0);
    assert_eq!(
        SemaNumber::Rational(BigRational::new(BigInt::one(), BigInt::from(4))).to_f64(),
        0.25
    );
    assert_eq!(SemaNumber::Real(1.5).to_f64(), 1.5);
}

#[test]
fn promote_to_common_level() {
    // Integer + Rational → both Rational
    let (a, b) = SemaNumber::promote(
        SemaNumber::Integer(BigInt::from(2)),
        SemaNumber::Rational(BigRational::new(BigInt::one(), BigInt::from(2))),
    );
    assert!(matches!(a, SemaNumber::Rational(_)));
    assert!(matches!(b, SemaNumber::Rational(_)));
    // Integer + Real → both Real
    let (a, b) = SemaNumber::promote(
        SemaNumber::Integer(BigInt::from(2)),
        SemaNumber::Real(0.5),
    );
    assert!(matches!(a, SemaNumber::Real(_)));
    assert!(matches!(b, SemaNumber::Real(_)));
}
```

- [ ] **Step 2: Implement `to_f64`, `level`, and `promote`**

Add to `number.rs`. `to_f64` needs `num-traits`' `ToPrimitive` for `BigInt`/`BigRational`:

```rust
use num_traits::ToPrimitive;

impl SemaNumber {
    /// Lossy projection to `f64` for inexact operations (`sqrt`, `sin`, mixed
    /// arithmetic). A `Complex` cannot project to a real — returns `f64::NAN`;
    /// callers that can receive complex must special-case it before calling.
    pub fn to_f64(&self) -> f64 {
        match self {
            SemaNumber::Integer(n) => n.to_f64().unwrap_or(f64::INFINITY),
            SemaNumber::Rational(r) => r.to_f64().unwrap_or(f64::INFINITY),
            SemaNumber::Real(f) => *f,
            SemaNumber::Complex(_) => f64::NAN,
        }
    }

    /// Tower level for promotion ordering.
    fn level(&self) -> u8 {
        match self {
            SemaNumber::Integer(_) => 0,
            SemaNumber::Rational(_) => 1,
            SemaNumber::Real(_) => 2,
            SemaNumber::Complex(_) => 3,
        }
    }

    /// Lift `self` up to the given level (never down). `Integer→Rational` is
    /// exact; `→Real` uses `to_f64`; `→Complex` pairs with an exact 0
    /// imaginary part.
    fn lift_to(self, level: u8) -> SemaNumber {
        match (self.level(), level) {
            (a, b) if a >= b => self,
            (0, 1) => match self {
                SemaNumber::Integer(n) => SemaNumber::Rational(BigRational::from(n)),
                _ => unreachable!(),
            },
            (_, 2) => SemaNumber::Real(self.to_f64()),
            (_, 3) => SemaNumber::Complex(Box::new(Complex {
                re: self,
                im: SemaNumber::Integer(BigInt::zero()),
            })),
            // (0,1) handled; (1,2)/(2,3) handled by the level==2/3 arms above.
            _ => self,
        }
    }

    /// Lift both operands to `max(level(a), level(b))` so a binary op has a
    /// single same-level case to implement per level.
    pub fn promote(a: SemaNumber, b: SemaNumber) -> (SemaNumber, SemaNumber) {
        let target = a.level().max(b.level());
        (a.lift_to(target), b.lift_to(target))
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p sema-core number::tests::to_f64_projection number::tests::promote_to_common_level`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber coercion (to_f64, promote to common level)"
```

### Task 0.5: Addition, subtraction, negation, multiplication

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`, `promote`, `normalize`.
- Produces: `SemaNumber::add(self, other) -> SemaNumber`, `::sub`, `::mul`, `::neg` — total functions (no error path; these cannot fail).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn add_sub_mul_neg() {
    let two = || SemaNumber::Integer(BigInt::from(2));
    let half = || SemaNumber::Rational(BigRational::new(BigInt::one(), BigInt::from(2)));
    // 2 + 1/2 = 5/2
    assert_eq!(two().add(half()).to_f64(), 2.5);
    // exact: result is Rational, not Real
    assert!(matches!(two().add(half()), SemaNumber::Rational(_)));
    // 1/2 + 1/2 = 1 (normalizes to Integer)
    assert!(matches!(half().add(half()), SemaNumber::Integer(n) if n == BigInt::one()));
    // 2 - 2 = 0
    assert!(matches!(two().sub(two()), SemaNumber::Integer(n) if n == BigInt::zero()));
    // 2 * 1/2 = 1
    assert!(matches!(two().mul(half()), SemaNumber::Integer(n) if n == BigInt::one()));
    // -(1/2) = -1/2
    assert_eq!(half().neg().to_f64(), -0.5);
    // contagion: 2 + 0.5 = 2.5 as Real
    assert!(matches!(two().add(SemaNumber::Real(0.5)), SemaNumber::Real(_)));
}
```

- [ ] **Step 2: Implement the operations**

Add to `number.rs`. Complex arithmetic operates component-wise (add/sub) and by the FOIL rule (mul), recursing into `SemaNumber` ops on the components:

```rust
impl SemaNumber {
    pub fn neg(self) -> SemaNumber {
        match self {
            SemaNumber::Integer(n) => SemaNumber::Integer(-n),
            SemaNumber::Rational(r) => SemaNumber::Rational(-r),
            SemaNumber::Real(f) => SemaNumber::Real(-f),
            SemaNumber::Complex(c) => SemaNumber::Complex(Box::new(Complex {
                re: c.re.neg(),
                im: c.im.neg(),
            })),
        }
        .normalize()
    }

    pub fn add(self, other: SemaNumber) -> SemaNumber {
        let (a, b) = SemaNumber::promote(self, other);
        match (a, b) {
            (SemaNumber::Integer(x), SemaNumber::Integer(y)) => SemaNumber::Integer(x + y),
            (SemaNumber::Rational(x), SemaNumber::Rational(y)) => SemaNumber::Rational(x + y),
            (SemaNumber::Real(x), SemaNumber::Real(y)) => SemaNumber::Real(x + y),
            (SemaNumber::Complex(x), SemaNumber::Complex(y)) => SemaNumber::Complex(Box::new(Complex {
                re: x.re.add(y.re),
                im: x.im.add(y.im),
            })),
            _ => unreachable!("promote guarantees equal levels"),
        }
        .normalize()
    }

    pub fn sub(self, other: SemaNumber) -> SemaNumber {
        self.add(other.neg())
    }

    pub fn mul(self, other: SemaNumber) -> SemaNumber {
        let (a, b) = SemaNumber::promote(self, other);
        match (a, b) {
            (SemaNumber::Integer(x), SemaNumber::Integer(y)) => SemaNumber::Integer(x * y),
            (SemaNumber::Rational(x), SemaNumber::Rational(y)) => SemaNumber::Rational(x * y),
            (SemaNumber::Real(x), SemaNumber::Real(y)) => SemaNumber::Real(x * y),
            (SemaNumber::Complex(x), SemaNumber::Complex(y)) => {
                // (a+bi)(c+di) = (ac - bd) + (ad + bc)i
                let ac = x.re.clone().mul(y.re.clone());
                let bd = x.im.clone().mul(y.im.clone());
                let ad = x.re.mul(y.im.clone());
                let bc = x.im.mul(y.re);
                SemaNumber::Complex(Box::new(Complex {
                    re: ac.sub(bd),
                    im: ad.add(bc),
                }))
            }
            _ => unreachable!("promote guarantees equal levels"),
        }
        .normalize()
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p sema-core number::tests::add_sub_mul_neg`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber add/sub/mul/neg with exactness contagion"
```

### Task 0.6: Division (exact when possible), with divide-by-zero signalling

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`, `promote`, `normalize`.
- Produces: `SemaNumber::div(self, other) -> Result<SemaNumber, DivByZero>` where `pub struct DivByZero;`. Exact/exact → exact rational (e.g. `1/3`); any inexact → real; complex division by the conjugate rule.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn division_is_exact_when_possible() {
    let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
    // 1 / 3 = 1/3 exact (NOT 0.333…)
    let third = n(1).div(n(3)).unwrap();
    assert!(matches!(&third, SemaNumber::Rational(r)
        if *r == BigRational::new(BigInt::one(), BigInt::from(3))));
    // 6 / 3 = 2 (normalizes to Integer)
    assert!(matches!(n(6).div(n(3)).unwrap(), SemaNumber::Integer(k) if k == BigInt::from(2)));
    // 1 / 2.0 = 0.5 (inexact contagion)
    assert!(matches!(n(1).div(SemaNumber::Real(2.0)).unwrap(), SemaNumber::Real(_)));
    // divide by exact zero → error
    assert!(n(1).div(n(0)).is_err());
    // divide by inexact zero → real infinity (IEEE), NOT an error
    assert!(matches!(n(1).div(SemaNumber::Real(0.0)).unwrap(), SemaNumber::Real(f) if f.is_infinite()));
}
```

- [ ] **Step 2: Implement `div`**

```rust
/// Returned by `SemaNumber::div` when dividing by an *exact* zero. An inexact
/// zero divisor follows IEEE-754 (→ ±inf / NaN), matching Scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DivByZero;

impl SemaNumber {
    pub fn div(self, other: SemaNumber) -> Result<SemaNumber, DivByZero> {
        // Guard exact-zero divisor up front (before promotion, so `1/0` and
        // `(1/2)/0` both signal, but `1/0.0` falls through to IEEE).
        if matches!(&other, SemaNumber::Integer(n) if n.is_zero())
            || matches!(&other, SemaNumber::Rational(r) if r.numer().is_zero())
        {
            return Err(DivByZero);
        }
        let (a, b) = SemaNumber::promote(self, other);
        let out = match (a, b) {
            // Integer/Integer → exact rational (reduces; normalize collapses to Integer if whole).
            (SemaNumber::Integer(x), SemaNumber::Integer(y)) => {
                SemaNumber::Rational(BigRational::new(x, y))
            }
            (SemaNumber::Rational(x), SemaNumber::Rational(y)) => SemaNumber::Rational(x / y),
            (SemaNumber::Real(x), SemaNumber::Real(y)) => SemaNumber::Real(x / y),
            (SemaNumber::Complex(x), SemaNumber::Complex(y)) => {
                // (a+bi)/(c+di) = ((a+bi)(c-di)) / (c²+d²)
                let denom = y.re.clone().mul(y.re.clone()).add(y.im.clone().mul(y.im.clone()));
                let num = SemaNumber::Complex(x).mul(SemaNumber::Complex(Box::new(Complex {
                    re: y.re,
                    im: y.im.neg(),
                })));
                match num {
                    SemaNumber::Complex(nc) => SemaNumber::Complex(Box::new(Complex {
                        re: nc.re.div(denom.clone())?,
                        im: nc.im.div(denom)?,
                    })),
                    // num collapsed to real (imaginary cancelled): divide directly.
                    real => real.div(denom)?,
                }
            }
            _ => unreachable!("promote guarantees equal levels"),
        };
        Ok(out.normalize())
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p sema-core number::tests::division_is_exact_when_possible`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber exact division with divide-by-zero signalling"
```

### Task 0.7: Comparison and equality across the tower

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`, `promote`.
- Produces: `SemaNumber::num_eq(&self, &other) -> bool` (numeric `=`, cross-level, exact); `SemaNumber::cmp_real(&self, &other) -> Option<Ordering>` (ordering for real operands; `None` if either is complex or NaN).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn compare_and_equal() {
    use std::cmp::Ordering;
    let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
    let half = SemaNumber::Rational(BigRational::new(BigInt::one(), BigInt::from(2)));
    // 1/2 = 0.5 across exact/inexact
    assert!(half.num_eq(&SemaNumber::Real(0.5)));
    // 2 = 2.0
    assert!(n(2).num_eq(&SemaNumber::Real(2.0)));
    // ordering
    assert_eq!(half.cmp_real(&n(1)), Some(Ordering::Less));
    assert_eq!(n(3).cmp_real(&n(2)), Some(Ordering::Greater));
    // exact bignum vs float above 2^53 stays exact (no lossy cast)
    let big = SemaNumber::Integer(BigInt::from(9_007_199_254_740_993_i64));
    assert_eq!(big.cmp_real(&SemaNumber::Real(9_007_199_254_740_992.0)), Some(Ordering::Greater));
    // complex is unordered
    let i = SemaNumber::Complex(Box::new(Complex { re: n(0), im: n(1) }));
    assert_eq!(i.cmp_real(&n(0)), None);
    assert!(!i.num_eq(&n(0)));
}
```

- [ ] **Step 2: Implement `num_eq` and `cmp_real`**

Exact/exact comparisons are done at the promoted level. The exact-vs-`Real` case must avoid the lossy `2^53` cast: convert the finite float to an exact `BigRational` and compare exactly.

```rust
use std::cmp::Ordering;

impl SemaNumber {
    /// Convert a finite `f64` to its exact rational value (no rounding). Used
    /// so exact-vs-inexact comparison never loses precision above 2^53.
    fn real_to_exact(f: f64) -> Option<SemaNumber> {
        if !f.is_finite() {
            return None;
        }
        // BigRational::from_float is exact for finite inputs.
        num_rational::BigRational::from_float(f).map(SemaNumber::Rational)
    }

    pub fn num_eq(&self, other: &SemaNumber) -> bool {
        match (self, other) {
            (SemaNumber::Complex(a), SemaNumber::Complex(b)) => {
                a.re.num_eq(&b.re) && a.im.num_eq(&b.im)
            }
            (SemaNumber::Complex(_), _) | (_, SemaNumber::Complex(_)) => false,
            _ => self.cmp_real(other) == Some(Ordering::Equal),
        }
    }

    /// Ordering for real numbers. `None` if either operand is complex or a NaN.
    /// Exact-vs-inexact converts the float to an exact rational so the compare
    /// is precise even above 2^53.
    pub fn cmp_real(&self, other: &SemaNumber) -> Option<Ordering> {
        if matches!(self, SemaNumber::Complex(_)) || matches!(other, SemaNumber::Complex(_)) {
            return None;
        }
        // If both inexact, compare as f64 (preserves NaN → None).
        if let (SemaNumber::Real(x), SemaNumber::Real(y)) = (self, other) {
            return x.partial_cmp(y);
        }
        // Mixed or both-exact: lift any Real to exact rational; if a Real is
        // non-finite, handle by sign.
        let to_exact = |v: &SemaNumber| -> Result<SemaNumber, Option<Ordering>> {
            match v {
                SemaNumber::Real(f) => {
                    if f.is_nan() {
                        Err(None)
                    } else if f.is_infinite() {
                        // Represent ±inf sentinel via Err carrying the sign later;
                        // handle below instead.
                        Err(Some(if *f > 0.0 { Ordering::Greater } else { Ordering::Less }))
                    } else {
                        Ok(SemaNumber::real_to_exact(*f).expect("finite"))
                    }
                }
                other => Ok(other.clone()),
            }
        };
        // Fast path for the infinity/NaN cases: if exactly one side is a
        // non-finite Real, its sign decides.
        match (self, other) {
            (SemaNumber::Real(f), _) if !f.is_finite() => {
                return if f.is_nan() { None } else if *f > 0.0 { Some(Ordering::Greater) } else { Some(Ordering::Less) };
            }
            (_, SemaNumber::Real(f)) if !f.is_finite() => {
                return if f.is_nan() { None } else if *f > 0.0 { Some(Ordering::Less) } else { Some(Ordering::Greater) };
            }
            _ => {}
        }
        let a = to_exact(self).ok()?;
        let b = to_exact(other).ok()?;
        let (a, b) = SemaNumber::promote(a, b);
        match (a, b) {
            (SemaNumber::Integer(x), SemaNumber::Integer(y)) => Some(x.cmp(&y)),
            (SemaNumber::Rational(x), SemaNumber::Rational(y)) => Some(x.cmp(&y)),
            _ => unreachable!("both exact after real_to_exact + promote"),
        }
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p sema-core number::tests::compare_and_equal`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber tower comparison and numeric equality (exact above 2^53)"
```

### Task 0.8: Display formatting (reader-round-trippable)

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`.
- Produces: `impl std::fmt::Display for SemaNumber` — `Integer` → decimal; `Rational` → `numer/denom`; `Real` → existing float rule (`2.0` keeps `.0`); `Complex` → `re±imi` (e.g. `3+4i`, `0-1i`, `2.0+0.5i`). The output must re-read to an equal value (Phase 3 reader).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn display_round_trippable() {
    let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
    assert_eq!(n(42).to_string(), "42");
    assert_eq!(
        SemaNumber::Rational(BigRational::new(BigInt::one(), BigInt::from(3))).to_string(),
        "1/3"
    );
    assert_eq!(SemaNumber::Real(2.0).to_string(), "2.0");
    assert_eq!(SemaNumber::Real(2.5).to_string(), "2.5");
    let c = SemaNumber::Complex(Box::new(Complex { re: n(3), im: n(4) }));
    assert_eq!(c.to_string(), "3+4i");
    let c2 = SemaNumber::Complex(Box::new(Complex { re: n(0), im: n(-1) }));
    assert_eq!(c2.to_string(), "0-1i");
}
```

- [ ] **Step 2: Implement `Display`**

The float arm must match the existing `Value` float rule exactly (`value.rs:2137`) so real numbers print identically whether they flow through `Value` or `SemaNumber`. The complex arm prints the imaginary part with an explicit sign; format the magnitude then prefix the sign so `0-1i` (not `0+-1i`).

```rust
use std::fmt;

/// Format a real component the way Sema prints floats/ints (shared by the
/// complex arm so `2.0+0.5i` matches standalone `2.0`/`0.5`).
fn fmt_real(n: &SemaNumber, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match n {
        SemaNumber::Integer(v) => write!(f, "{v}"),
        SemaNumber::Rational(r) => write!(f, "{}/{}", r.numer(), r.denom()),
        SemaNumber::Real(v) => {
            if v.fract() == 0.0 && v.is_finite() {
                write!(f, "{v:.1}")
            } else {
                write!(f, "{v}")
            }
        }
        SemaNumber::Complex(_) => unreachable!("complex component is never complex"),
    }
}

impl fmt::Display for SemaNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemaNumber::Complex(c) => {
                fmt_real(&c.re, f)?;
                // Explicit sign then magnitude, so `0-1i` reads back correctly.
                let (sign, mag) = match &c.im {
                    SemaNumber::Integer(v) if *v < BigInt::zero() => ('-', SemaNumber::Integer(-v.clone())),
                    SemaNumber::Rational(r) if *r < BigRational::from(BigInt::zero()) => ('-', SemaNumber::Rational(-r.clone())),
                    SemaNumber::Real(v) if v.is_sign_negative() => ('-', SemaNumber::Real(-v)),
                    other => ('+', other.clone()),
                };
                write!(f, "{sign}")?;
                fmt_real(&mag, f)?;
                write!(f, "i")
            }
            real => fmt_real(real, f),
        }
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p sema-core number::tests::display_round_trippable`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber Display (reader-round-trippable, matches float rule)"
```

### Task 0.9: Exactness conversions and integer/float bridges

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`.
- Produces: `SemaNumber::to_inexact(self) -> SemaNumber` (every component → `Real`); `SemaNumber::to_exact(self) -> SemaNumber` (finite `Real` → exact `Rational`/`Integer`; already-exact unchanged); `SemaNumber::from_i64(i64)`, `SemaNumber::from_f64(f64)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn exactness_conversions() {
    let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
    // exact → inexact
    assert!(matches!(n(3).to_inexact(), SemaNumber::Real(f) if f == 3.0));
    // inexact 0.5 → exact 1/2
    assert!(matches!(SemaNumber::Real(0.5).to_exact(),
        SemaNumber::Rational(r) if r == BigRational::new(BigInt::one(), BigInt::from(2))));
    // inexact 2.0 → exact 2 (normalizes to Integer)
    assert!(matches!(SemaNumber::Real(2.0).to_exact(), SemaNumber::Integer(k) if k == BigInt::from(2)));
    // bridges
    assert!(matches!(SemaNumber::from_i64(5), SemaNumber::Integer(k) if k == BigInt::from(5)));
    assert!(matches!(SemaNumber::from_f64(1.5), SemaNumber::Real(f) if f == 1.5));
}
```

- [ ] **Step 2: Implement the conversions**

```rust
impl SemaNumber {
    pub fn from_i64(v: i64) -> SemaNumber {
        SemaNumber::Integer(BigInt::from(v))
    }
    pub fn from_f64(v: f64) -> SemaNumber {
        SemaNumber::Real(v)
    }

    pub fn to_inexact(self) -> SemaNumber {
        match self {
            SemaNumber::Complex(c) => SemaNumber::Complex(Box::new(Complex {
                re: c.re.to_inexact(),
                im: c.im.to_inexact(),
            })),
            other => SemaNumber::Real(other.to_f64()),
        }
    }

    /// Convert inexact components to their exact rational value. Non-finite
    /// reals have no exact value and are left as-is (callers that require
    /// exactness should error; R7RS `inexact->exact` on ±inf/NaN is undefined).
    pub fn to_exact(self) -> SemaNumber {
        match self {
            SemaNumber::Real(f) => {
                SemaNumber::real_to_exact(f).map(|n| n.normalize()).unwrap_or(SemaNumber::Real(f))
            }
            SemaNumber::Complex(c) => SemaNumber::Complex(Box::new(Complex {
                re: c.re.to_exact(),
                im: c.im.to_exact(),
            }))
            .normalize(),
            exact => exact,
        }
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p sema-core number::tests::exactness_conversions`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber exactness conversions and i64/f64 bridges"
```

### Task 0.10: Parse `SemaNumber` from a literal string (integers, rationals, radix)

**Files:**
- Modify: `crates/sema-core/src/number.rs`

**Interfaces:**
- Consumes: `SemaNumber`.
- Produces: `SemaNumber::parse_int_radix(digits: &str, radix: u32) -> Option<SemaNumber>` (arbitrary-precision, sign-aware); `SemaNumber::parse_rational(s: &str) -> Option<SemaNumber>` (parses `a/b`). These back the reader in Phases 2 & 4 and `string->number` in Phase 5.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn parse_literals() {
    // arbitrary-precision decimal beyond i64
    let big = SemaNumber::parse_int_radix("170141183460469231731687303715884105728", 10).unwrap();
    assert!(matches!(big, SemaNumber::Integer(_)));
    // hex / binary
    assert!(matches!(SemaNumber::parse_int_radix("ff", 16).unwrap(),
        SemaNumber::Integer(n) if n == BigInt::from(255)));
    assert!(matches!(SemaNumber::parse_int_radix("-101", 2).unwrap(),
        SemaNumber::Integer(n) if n == BigInt::from(-5)));
    // rational
    assert!(matches!(SemaNumber::parse_rational("22/7").unwrap(),
        SemaNumber::Rational(r) if r == BigRational::new(BigInt::from(22), BigInt::from(7))));
    // 6/3 → normalizes to Integer 2
    assert!(matches!(SemaNumber::parse_rational("6/3").unwrap(), SemaNumber::Integer(n) if n == BigInt::from(2)));
    // rejects garbage
    assert!(SemaNumber::parse_rational("1/0").is_none()); // zero denominator
    assert!(SemaNumber::parse_int_radix("xyz", 16).is_none());
}
```

- [ ] **Step 2: Implement the parsers**

```rust
use num_bigint::Sign;
use std::str::FromStr;

impl SemaNumber {
    /// Parse an integer of arbitrary size in the given radix (2..=36). Accepts
    /// an optional leading `+`/`-`. Returns `None` on any invalid digit.
    pub fn parse_int_radix(digits: &str, radix: u32) -> Option<SemaNumber> {
        let (sign, body) = match digits.strip_prefix('-') {
            Some(rest) => (Sign::Minus, rest),
            None => (Sign::Plus, digits.strip_prefix('+').unwrap_or(digits)),
        };
        if body.is_empty() {
            return None;
        }
        let bytes = body.as_bytes();
        let magnitude = num_bigint::BigUint::parse_bytes(bytes, radix)?;
        Some(SemaNumber::Integer(BigInt::from_biguint(sign, magnitude)).normalize())
    }

    /// Parse `numer/denom` (decimal, sign on the numerator). `None` on a zero
    /// denominator or invalid digits.
    pub fn parse_rational(s: &str) -> Option<SemaNumber> {
        let (n, d) = s.split_once('/')?;
        let numer = BigInt::from_str(n).ok()?;
        let denom = BigInt::from_str(d).ok()?;
        if denom.is_zero() {
            return None;
        }
        Some(SemaNumber::Rational(BigRational::new(numer, denom)).normalize())
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p sema-core number::tests::parse_literals`
Expected: PASS.

- [ ] **Step 4: Run the whole Phase-0 module and lint**

Run: `cargo test -p sema-core number:: && jake lint`
Expected: all `number::` tests PASS; lint clean.

- [ ] **Step 5: Commit**

```bash
git add crates/sema-core/src/number.rs
git commit -m "core: SemaNumber literal parsers (radix integers, rationals)"
```

**Phase 0 exit criteria:** `cargo test -p sema-core number::` is green with the full tower — arithmetic, exact division, cross-level comparison, exactness conversions, display, parsing — proven with zero involvement of the runtime. Every later phase builds on this tested core.

## Phase 1 — Bignums in `Value` (overflow promotes instead of raising)

Wire arbitrary-precision integers into the runtime. After this phase, `(* 1000000000000 1000000000000)` and `(factorial 100)` produce correct big integers instead of raising "integer overflow". This phase also builds the `as_number`/`from_number` bridge that Phases 2–3 extend, and exercises the full NaN-box trait surface once (so Phases 2–3 are mechanical repeats).

### Task 1.1: Add `TAG_BIGINT`, the `BigInt` view variant, and all core trait arms

**Files:**
- Modify: `crates/sema-core/src/value.rs`

**Interfaces:**
- Produces: `Value::from_bigint(BigInt) -> Value` (normalizes: i64-range → `Value::int`, else `TAG_BIGINT`); `Value::as_bigint(&self) -> Option<BigInt>` (any integer → `BigInt`; `None` for non-integers); `Value::is_bigint(&self) -> bool`; `Value::as_number(&self) -> Option<SemaNumber>`; `Value::from_number(SemaNumber) -> Value`. New `ValueView::BigInt(Rc<BigInt>)` and `ValueViewRef::BigInt(&BigInt)`.

Adding an enum variant forces every exhaustive `match` on these enums to compile — this task lands the variant plus ALL arms in `value.rs` at once, so the crate compiles, then verifies with a unit test.

- [ ] **Step 1: Write the failing test**

Add near the bottom of `value.rs` (in its `#[cfg(test)] mod tests`, or create one):

```rust
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
```

- [ ] **Step 2: Add imports, the tag, the variant, and constructors**

At the top of `value.rs`, add:

```rust
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use crate::number::{Complex as SemaComplex, SemaNumber};
```

After `const TAG_CHANNEL: u64 = 29;` (line ~546) add:

```rust
const TAG_BIGINT: u64 = 30;
```

In `enum ValueView` (after the `Int(i64)` line, line ~615) add:

```rust
    BigInt(Rc<BigInt>),
```

In `enum ValueViewRef<'a>` (after its `Int(i64)` line, line ~651) add:

```rust
    BigInt(&'a BigInt),
```

In the heap constructors block (after `int`/`float`, near line ~745), add:

```rust
    /// Construct an integer of any magnitude, normalizing to the tightest
    /// representation: values in i64 range become a fixnum/int-big, larger
    /// values are heap-boxed under `TAG_BIGINT`.
    pub fn from_bigint(n: BigInt) -> Value {
        match n.to_i64() {
            Some(i) => Value::int(i),
            None => Value::from_rc_ptr(TAG_BIGINT, Rc::new(n)),
        }
    }
```

- [ ] **Step 3: Add all dispatch/trait arms (this is what makes it compile)**

Add a `TAG_BIGINT` arm to each of the following sites in `value.rs`, mirroring the existing `TAG_STRING`/`TAG_INT_BIG` leaf pattern:

1. **`view()`** (after the `TAG_INT_BIG` arm, ~line 1083):
   ```rust
   TAG_BIGINT => ValueView::BigInt(unsafe { self.get_rc::<BigInt>() }),
   ```
2. **`view_ref()`** (after its `TAG_INT_BIG` arm, ~line 1154):
   ```rust
   TAG_BIGINT => ValueViewRef::BigInt(unsafe { self.borrow_ref::<BigInt>() }),
   ```
3. **`heap_strong_count()`** count_at table (after `TAG_INT_BIG`, ~line 1223):
   ```rust
   TAG_BIGINT => count_at::<BigInt>(ptr),
   ```
4. **`type_name()`** — extend the existing int arm so bignum reports `"int"` (~line 1261):
   ```rust
   TAG_INT_SMALL | TAG_INT_BIG | TAG_BIGINT => "int",
   ```
5. **`Clone`** increment table (after `TAG_INT_BIG`, ~line 1830):
   ```rust
   TAG_BIGINT => Rc::increment_strong_count(ptr as *const BigInt),
   ```
6. **`Drop`** table (after `TAG_INT_BIG`, ~line 1884):
   ```rust
   TAG_BIGINT => drop(Rc::from_raw(ptr as *const BigInt)),
   ```
7. **`Display`** (`fmt::Display`, add arm ~line 2136):
   ```rust
   ValueViewRef::BigInt(n) => write!(f, "{n}"),
   ```
8. **`Debug`** (`fmt::Debug`, uses `view()`, add arm ~line 2423):
   ```rust
   ValueView::BigInt(n) => write!(f, "Int({n})"),
   ```
9. **`PartialEq`** (in the `view_ref` match, add arm; a bignum is outside i64 range so it can only equal another equal bignum):
   ```rust
   (ValueViewRef::BigInt(a), ValueViewRef::BigInt(b)) => a == b,
   ```
10. **`Hash`** (add arm; distinct discriminant so it never collides with a fixnum — they are disjoint by construction):
    ```rust
    ValueViewRef::BigInt(n) => {
        30u8.hash(state);
        n.hash(state);
    }
    ```
11. **`Ord`**: in `type_order`, give bignum the same bucket as int so mixed-size integers sort numerically (add arm before the `_`):
    ```rust
    ValueViewRef::BigInt(_) => 2,
    ```
    and in the main `cmp` match, add numeric cross-arms (after the `(Int,Int)` arm):
    ```rust
    (ValueViewRef::BigInt(a), ValueViewRef::BigInt(b)) => a.cmp(b),
    (ValueViewRef::Int(a), ValueViewRef::BigInt(b)) => BigInt::from(a).cmp(b),
    (ValueViewRef::BigInt(a), ValueViewRef::Int(b)) => a.as_ref().cmp(&BigInt::from(b)),
    ```

- [ ] **Step 4: Add `as_bigint`, `is_bigint`, and the tower bridge**

In the accessors `impl Value` block, add:

```rust
    #[inline(always)]
    pub fn is_bigint(&self) -> bool {
        is_boxed(self.0) && get_tag(self.0) == TAG_BIGINT
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

    /// Lift any numeric Value into the tower type for arithmetic. `None` for
    /// non-numbers. (Extended with Rational/Complex arms in Phases 2–3.)
    pub fn as_number(&self) -> Option<SemaNumber> {
        match self.view_ref() {
            ValueViewRef::Int(n) => Some(SemaNumber::from_i64(n)),
            ValueViewRef::BigInt(n) => Some(SemaNumber::Integer(n.clone())),
            ValueViewRef::Float(f) => Some(SemaNumber::Real(f)),
            _ => None,
        }
    }

    /// Lower a tower number to the tightest Value. (Extended with Rational/
    /// Complex arms in Phases 2–3.)
    pub fn from_number(n: SemaNumber) -> Value {
        match n.normalize() {
            SemaNumber::Integer(big) => Value::from_bigint(big),
            SemaNumber::Real(f) => Value::float(f),
            SemaNumber::Rational(_) | SemaNumber::Complex(_) => {
                unreachable!("rational/complex Values are introduced in Phases 2–3")
            }
        }
    }
```

- [ ] **Step 5: Build and run the test**

Run: `cargo build -p sema-core && cargo test -p sema-core value::tests::bigint_roundtrip_and_normalize`
Expected: compiles (all match sites now exhaustive), test PASS.

- [ ] **Step 6: Update the `cycle.rs` leaf doc-comment**

In `crates/sema-core/src/cycle.rs`, `NodePtr::of_value`'s doc comment lists leaf types — append "bignums, rationals, complex" to the parenthetical (no logic change; `value_node_ptr`'s `_ => None` already excludes them). Verify:

Run: `cargo test -p sema-core` (full core suite, incl. GC tests)
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/sema-core/src/value.rs crates/sema-core/src/cycle.rs
git commit -m "core: add TAG_BIGINT, BigInt view variant, and tower bridge (as_number/from_number)"
```

### Task 1.2: Serialize/deserialize bignum constants in bytecode

**Files:**
- Modify: `crates/sema-vm/src/serialize.rs`
- Modify: `website/docs/internals/bytecode-format.md`
- Test: `crates/sema-vm/src/serialize.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Value::from_bigint`, `ValueView::BigInt`.
- Produces: `VAL_BIGINT = 0x0D` constant kind, round-trippable through `serialize_value`/`deserialize_value`.

- [ ] **Step 1: Write the failing test**

Add to the tests module in `serialize.rs` (follow the existing round-trip test style there):

```rust
#[test]
fn bigint_constant_roundtrips() {
    use num_bigint::BigInt;
    use std::str::FromStr;
    let n = BigInt::from_str("170141183460469231731687303715884105728").unwrap();
    let val = sema_core::Value::from_bigint(n);
    let mut stb = StringTableBuilder::default();
    let mut buf = Vec::new();
    serialize_value(&val, &mut buf, &mut stb).unwrap();
    let table = stb.finish();
    let remap = build_remap_table(&table);
    let mut cursor = 0;
    let back = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
    assert_eq!(back, val);
    assert_eq!(cursor, buf.len());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p sema-vm serialize::tests::bigint_constant_roundtrips`
Expected: FAIL — `serialize_value` hits its `_ =>` "cannot serialize" arm for `ValueView::BigInt`.

- [ ] **Step 3: Add the constant kind and both directions**

In `serialize.rs`, after `const VAL_BYTEVECTOR: u8 = 0x0C;` add:

```rust
const VAL_BIGINT: u8 = 0x0D;
```

In `serialize_value`, add an arm before the catch-all `_`:

```rust
ValueView::BigInt(n) => {
    buf.push(VAL_BIGINT);
    let bytes = n.to_signed_bytes_le();
    let len = checked_u32(bytes.len(), "bigint byte length")?;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&bytes);
}
```

In `deserialize_value_inner`, add an arm (after `VAL_FLOAT`):

```rust
VAL_BIGINT => {
    let len = read_u32_le(buf, cursor)? as usize;
    let bytes = read_bytes(buf, cursor, len)?;
    Ok(Value::from_bigint(num_bigint::BigInt::from_signed_bytes_le(&bytes)))
}
```

Add `use num_bigint::BigInt;` if not already imported (the code uses the fully-qualified path above, so no import needed).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p sema-vm serialize::tests::bigint_constant_roundtrips`
Expected: PASS.

- [ ] **Step 5: Document the constant kind**

In `website/docs/internals/bytecode-format.md`, in the value-constant kinds table, add a row: `0x0D VAL_BIGINT` — `u32 LE byte-length, then that many bytes of two's-complement little-endian magnitude (num-bigint `to_signed_bytes_le`)`.

- [ ] **Step 6: Commit**

```bash
git add crates/sema-vm/src/serialize.rs website/docs/internals/bytecode-format.md
git commit -m "vm: serialize bignum constants (VAL_BIGINT) + format spec"
```

### Task 1.3: Stdlib arithmetic promotes to bignum on overflow

**Files:**
- Modify: `crates/sema-stdlib/src/arithmetic.rs`
- Test: `crates/sema/tests/eval_test.rs`

**Interfaces:**
- Consumes: `Value::as_number`, `Value::from_number` (tower bridge).
- Produces: `+ - *` never raise "integer overflow"; they promote through `SemaNumber`. Exactness and existing int/float behavior unchanged for in-range values.

The current variadic `+ - *` loops use `checked_add`/`checked_mul` and raise `overflow(...)` on wrap. Replace the overflow arm with a tower fold: keep the fast i64/f64 accumulator, and on `checked_*` returning `None`, switch the accumulator to a `SemaNumber` and continue folding via the tower. Simplest correct approach: fold the whole operation through `SemaNumber` when any operand is a bignum OR an i64 op overflows.

- [ ] **Step 1: Write the failing tests**

Add to `eval_test.rs` (in the `eval_tests!` block):

```rust
// bignum promotion on overflow
"(* 1000000000000 1000000000000)" => common::eval_tw("1000000000000000000000000"),
"(+ 9223372036854775807 1)" => common::eval_tw("9223372036854775808"),
"(- -9223372036854775808 1)" => common::eval_tw("-9223372036854775809"),
// factorial-style product stays exact
"(let loop ((i 1) (acc 1)) (if (> i 25) acc (loop (+ i 1) (* acc i))))"
    => common::eval_tw("15511210043330985984000000"),
// mixing bignum with float is inexact contagion
"(+ 1000000000000000000000000 0.0)" => common::eval_tw("1e24"),
// in-range arithmetic is byte-identical to before
"(+ 2 3)" => common::eval_tw("5"),
"(* 6 7)" => common::eval_tw("42"),
```

Note: `common::eval_tw("1000000000000000000000000")` requires the reader to parse a > i64 integer literal into a bignum. That is Task 1.4 below. **Order 1.4 before running these tests, or temporarily construct the expected via a bignum expression.** (Subagent note: implement 1.4's reader change first if executing strictly TDD; the two tasks are co-dependent for the test oracle.)

- [ ] **Step 2: Implement tower promotion in `+`**

Replace the `overflow("+")` path. The cleanest total rewrite of `+` that preserves the fast path:

```rust
register_fn(env, "+", |args| {
    if args.is_empty() {
        return Ok(Value::int(0));
    }
    let mut has_float = false;
    let mut int_sum: i64 = 0;
    let mut float_sum: f64 = 0.0;
    let mut tower: Option<SemaNumber> = None; // engaged once we leave i64 range
    for arg in args {
        // Once in tower mode, everything folds through the tower.
        if let Some(acc) = tower.take() {
            let n = arg
                .as_number()
                .ok_or_else(|| SemaError::type_error("number", arg.type_name()))?;
            tower = Some(acc.add(n));
            continue;
        }
        match arg.view_ref() {
            ValueViewRef::Int(n) => {
                if has_float {
                    float_sum += n as f64;
                } else {
                    match int_sum.checked_add(n) {
                        Some(s) => int_sum = s,
                        None => {
                            // Overflow: switch to the tower, seeded with the
                            // running int sum plus this operand.
                            tower = Some(SemaNumber::from_i64(int_sum).add(SemaNumber::from_i64(n)));
                        }
                    }
                }
            }
            ValueViewRef::Float(fv) => {
                if !has_float {
                    float_sum = int_sum as f64;
                    has_float = true;
                }
                float_sum += fv;
            }
            ValueViewRef::BigInt(_) => {
                // A bignum operand: enter tower mode seeded with the running sum.
                let seed = if has_float {
                    SemaNumber::Real(float_sum)
                } else {
                    SemaNumber::from_i64(int_sum)
                };
                let n = arg.as_number().unwrap();
                tower = Some(seed.add(n));
            }
            _ => return Err(SemaError::type_error("number", arg.type_name())),
        }
    }
    if let Some(acc) = tower {
        Ok(Value::from_number(acc))
    } else if has_float {
        Ok(Value::float(float_sum))
    } else {
        Ok(Value::int(int_sum))
    }
});
```

Add `use sema_core::number::SemaNumber;` at the top of `arithmetic.rs`.

- [ ] **Step 3: Apply the same pattern to `-` and `*`**

Mirror the tower-fold structure in `-` (seed tower with the running result; subsequent operands use `acc.sub(n)`; the unary-negation single-arg case uses `arg.as_number()?.neg()` on overflow of `checked_neg`) and `*` (`acc.mul(n)`, seed `1`). For `*`, the `checked_mul` `None` branch seeds `tower = Some(SemaNumber::from_i64(int_prod).mul(SemaNumber::from_i64(n)))`. Keep the empty-args identities (`0` for `+`/`-`-unary?, `1` for `*`).

Keep `mod`/`modulo` as-is for now (integer-only; bignum `mod` is added in Phase 5). If a bignum reaches `mod`, its existing `_ => type_error` arm fires — acceptable until Phase 5, but add a follow-up note.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sema --test eval_test -- bignum` (and the specific added cases)
Expected: PASS (after Task 1.4's reader change lands).

- [ ] **Step 5: Commit**

```bash
git add crates/sema-stdlib/src/arithmetic.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: + - * promote to bignum on i64 overflow (no more overflow error)"
```

### Task 1.4: Reader parses out-of-range integer literals as bignums

**Files:**
- Modify: `crates/sema-reader/src/lexer.rs`
- Modify: `crates/sema-reader/src/reader.rs`
- Test: `crates/sema-reader/src/lexer.rs` (tests module) and `crates/sema/tests/eval_test.rs`

**Interfaces:**
- Consumes: `SemaNumber::parse_int_radix`, `Value::from_bigint`.
- Produces: a decimal integer literal that overflows `i64` lexes to a new `Token::BigInt(BigInt)` and parses to a bignum `Value`, instead of the current "invalid integer" reader error.

Currently `read_number` does `s.parse::<i64>()` and errors on overflow (`lexer.rs:781`). Add a `Token::BigInt(num_bigint::BigInt)` variant and fall back to it when the `i64` parse overflows (only for pure-integer, non-float literals).

- [ ] **Step 1: Write the failing test**

In `lexer.rs` tests:

```rust
#[test]
fn out_of_range_integer_lexes_as_bigint() {
    use num_bigint::BigInt;
    use std::str::FromStr;
    let first = |src: &str| tokenize(src).unwrap().into_iter().next().unwrap().token;
    assert_eq!(
        first("170141183460469231731687303715884105728"),
        Token::BigInt(BigInt::from_str("170141183460469231731687303715884105728").unwrap())
    );
    // in-range still lexes as Int
    assert_eq!(first("42"), Token::Int(42));
    assert_eq!(first("-9223372036854775808"), Token::Int(i64::MIN));
    // one past i64::MAX is a bignum
    assert_eq!(
        first("9223372036854775808"),
        Token::BigInt(BigInt::from_str("9223372036854775808").unwrap())
    );
}
```

- [ ] **Step 2: Add the token variant and the fallback**

Add to the `Token` enum in `lexer.rs` (near `Int(i64)`, line ~22):

```rust
    BigInt(num_bigint::BigInt),
```

In `read_number`, replace the integer branch (lines ~780–786):

```rust
} else {
    match s.parse::<i64>() {
        Ok(n) => Ok((Token::Int(n), i)),
        Err(_) => {
            // Out of i64 range: parse as an arbitrary-precision integer.
            let big = num_bigint::BigInt::parse_bytes(s.as_bytes(), 10).ok_or_else(|| {
                SemaError::Reader { message: format!("invalid integer: {s}"), span: *span }
            })?;
            Ok((Token::BigInt(big), i))
        }
    }
}
```

Add `num-bigint` to `crates/sema-reader/Cargo.toml` `[dependencies]` (`num-bigint = "0.4"`).

- [ ] **Step 3: Consume the token in the parser**

In `reader.rs`, find where `Token::Int(n)` becomes `Value::int(n)` (the parser's atom dispatch, ~line 452). Add alongside it:

```rust
Token::BigInt(n) => Value::from_bigint(n.clone()),
```

(If the match consumes tokens by value, use `Token::BigInt(n) => Value::from_bigint(n)`.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sema-reader out_of_range_integer && cargo test -p sema --test eval_test -- bignum`
Expected: PASS (this unblocks Task 1.3's tests too).

- [ ] **Step 5: Commit**

```bash
git add crates/sema-reader/src/lexer.rs crates/sema-reader/src/reader.rs crates/sema-reader/Cargo.toml Cargo.lock
git commit -m "reader: lex out-of-range integer literals as bignums"
```

### Task 1.5: VM fast-path opcodes and helpers promote on overflow

**Files:**
- Modify: `crates/sema-vm/src/vm.rs`
- Test: `crates/sema/tests/eval_test.rs`

**Interfaces:**
- Consumes: `Value::as_number`, `Value::from_number`.
- Produces: `vm_add/vm_sub/vm_mul` promote to bignum instead of raising `int_overflow`; inline `ADD_INT`/`SUB_INT`/`MUL_INT` opcodes promote when their `checked_*` overflows; `vm_eq`/`vm_lt` handle bignum operands.

The VM has two layers: inline specialized opcodes (`ADD_INT` etc., which operate on small-int immediates and call `Value::int`) and generic helpers (`vm_add` etc.). Both currently raise on overflow. Because these run in the hottest loop, keep the i64 fast path and only fall to the tower on the overflow/non-fixnum branch.

- [ ] **Step 1: Write the failing tests**

Add to `eval_test.rs`:

```rust
// These exercise the VM's inline opcodes and generic helpers under overflow.
"(let ((a 9223372036854775807)) (+ a a))" => common::eval_tw("18446744073709551614"),
"(let ((a 9223372036854775807)) (* a a))" => common::eval_tw("85070591730234615847396907784232501249"),
"(< 9223372036854775807 9223372036854775808)" => common::eval_tw("#t"),
"(= 9223372036854775808 9223372036854775808)" => common::eval_tw("#t"),
```

- [ ] **Step 2: Update the generic helpers**

In `vm.rs`, rewrite `vm_add` (line ~4332). Keep the 4-arm int/float match; replace the `int_overflow` raise with a tower fallthrough, and add bignum handling by delegating any non-`(Int,Int)`/non-float combination to the tower:

```rust
fn vm_add(a: &Value, b: &Value) -> Result<Value, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => match x.checked_add(y) {
            Some(s) => Ok(Value::int(s)),
            None => Ok(Value::from_number(
                SemaNumber::from_i64(x).add(SemaNumber::from_i64(y)),
            )),
        },
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => Ok(Value::float(x + y)),
        (ValueViewRef::Int(x), ValueViewRef::Float(y)) => Ok(Value::float(x as f64 + y)),
        (ValueViewRef::Float(x), ValueViewRef::Int(y)) => Ok(Value::float(x + y as f64)),
        _ => {
            // Bignum/rational/complex (rational/complex arrive in later phases):
            // fold through the tower if both are numbers.
            match (a.as_number(), b.as_number()) {
                (Some(x), Some(y)) => Ok(Value::from_number(x.add(y))),
                _ => Err(SemaError::type_error("number", a.type_name())),
            }
        }
    }
}
```

Apply the identical shape to `vm_sub` (`.sub`), `vm_mul` (`.mul`, with `checked_mul`). For `vm_div` (line ~4396): keep the current exact-int / float behavior for now (rationals are Phase 2); just add the `_ => tower` fallthrough so bignum division doesn't type-error — for Phase 1, bignum `/` should go through `SemaNumber::div` and (since Phase 1 `from_number` can't lower a Rational yet) — **defer bignum `/` to Phase 2**; for now add a bignum arm that converts to float division to avoid a type error:
```rust
// Phase-1 stopgap (replaced in Phase 2 with exact rational division):
_ => match (a.as_number(), b.as_number()) {
    (Some(x), Some(y)) => Ok(Value::float(x.to_f64() / y.to_f64())),
    _ => Err(SemaError::type_error("number", a.type_name())),
},
```

- [ ] **Step 3: Update `vm_eq` and `vm_lt`**

`vm_eq` (line ~4420): add a bignum-aware fallthrough after the existing int/float arms:

```rust
fn vm_eq(a: &Value, b: &Value) -> bool {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => x == y,
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => x == y,
        (ValueViewRef::Int(x), ValueViewRef::Float(y))
        | (ValueViewRef::Float(y), ValueViewRef::Int(x)) => {
            sema_core::num::cmp_int_float(x, y) == Some(std::cmp::Ordering::Equal)
        }
        _ => match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) => x.num_eq(&y),
            _ => a == b, // fall back to structural equality for non-numbers
        },
    }
}
```

`vm_lt` (line ~4432): add a tower fallthrough returning `Ok(x.cmp_real(&y) == Some(Ordering::Less))` for the non-int/float case (error if either is non-number).

- [ ] **Step 4: Promote in the inline opcodes**

Find the `ADD_INT` opcode dispatch (line ~2252), which sign-extends 45-bit payloads and calls `Value::int`. It currently uses `checked_add` and raises `int_overflow` (via `MUL_INT` at ~2333). Change each of `ADD_INT`/`SUB_INT`/`MUL_INT` so the `checked_*` `None` branch calls `Value::from_number(SemaNumber::from_i64(x).<op>(SemaNumber::from_i64(y)))` and pushes that, instead of raising. (These operands are always small-int immediates, so `SemaNumber::from_i64` is exact.)

Add `use sema_core::number::SemaNumber;` to `vm.rs` if absent.

- [ ] **Step 5: Run the tests + bytecode smoke**

Run: `cargo test -p sema --test eval_test && jake smoke-bytecode`
Expected: PASS; disasm/run of every example still matches.

- [ ] **Step 6: Commit**

```bash
git add crates/sema-vm/src/vm.rs crates/sema/tests/eval_test.rs
git commit -m "vm: fast-path opcodes and helpers promote to bignum on overflow"
```

### Task 1.6: `integer?`/`number?` recognize bignums; overflow-error test removed

**Files:**
- Modify: `crates/sema-stdlib/src/predicates.rs`
- Modify: `crates/sema/tests/eval_test.rs` (and remove/replace any test that asserted an overflow error)

**Interfaces:**
- Consumes: `Value::is_bigint`, `Value::is_int`, `Value::is_float`.
- Produces: `(integer? <bignum>)` → `#t`, `(number? <bignum>)` → `#t`.

- [ ] **Step 1: Write the failing test**

```rust
"(integer? 170141183460469231731687303715884105728)" => common::eval_tw("#t"),
"(number? 170141183460469231731687303715884105728)" => common::eval_tw("#t"),
"(integer? 2.0)" => common::eval_tw("#t"),
"(integer? 2.5)" => common::eval_tw("#f"),
```

- [ ] **Step 2: Fix the predicates**

`number?` (line ~37) currently checks `is_int() || is_float()`. Update to also accept bignum (and, forward-looking, any numeric view). Simplest robust definition using `as_number`:

```rust
register_fn(env, "number?", |args| {
    check_arity!(args, "number?", 1);
    Ok(Value::bool(args[0].as_number().is_some()))
});
register_fn(env, "integer?", |args| {
    check_arity!(args, "integer?", 1);
    let is_int = args[0].is_int()
        || args[0].is_bigint()
        || matches!(args[0].as_float(), Some(f) if f.is_finite() && f.fract() == 0.0 && args[0].is_float());
    Ok(Value::bool(is_int))
});
```

(`float?` stays `is_float()`.)

- [ ] **Step 3: Purge the stale overflow-error assertion**

Search the test suite for the old overflow behavior and update it:

Run: `grep -rn "integer overflow\|exceeds i64" crates/*/tests crates/*/src`
For each hit that asserts overflow *raises*, replace with the new promote-to-bignum expectation. The `overflow()` helper in `arithmetic.rs` is now dead if `+ - *` no longer call it — remove it and its `use` if `cargo build` warns it is unused (keep it only if `mod` still references it).

- [ ] **Step 4: Run the tests + lint**

Run: `cargo test -p sema --test eval_test && jake lint`
Expected: PASS; no unused-code warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/sema-stdlib/src/predicates.rs crates/sema-stdlib/src/arithmetic.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: integer?/number? recognize bignums; drop overflow-raise path"
```

**Phase 1 exit criteria:** `(factorial 100)`, `(* big big)`, `(+ i64::MAX 1)` all produce correct arbitrary-precision integers via both the VM fast path and the stdlib functions; bignum literals read, print, serialize, compare, and hash correctly; `jake test && jake smoke-bytecode && jake lint` are green. `docs/limitations.md`'s bignum claim is now false (updated in Phase 6).

## Phase 2 — Exact rationals

After this phase, `(/ 1 3)` → `1/3` (exact), `(+ 1/2 1/3)` → `5/6`, and `1/3` is a readable literal. Rationals are a leaf heap type under `TAG_RATIONAL`, wired exactly like bignums in Phase 1.

### Task 2.1: Add `TAG_RATIONAL`, the `Rational` view variant, trait arms, and serialization

**Files:**
- Modify: `crates/sema-core/src/value.rs`
- Modify: `crates/sema-vm/src/serialize.rs` + `website/docs/internals/bytecode-format.md`

**Interfaces:**
- Produces: `Value::rational(BigRational) -> Value` (normalizes: integer-valued → `from_bigint`, else `TAG_RATIONAL`); `Value::as_rational(&self) -> Option<BigRational>`; `Value::is_rational(&self)`; new `ValueView::Rational(Rc<BigRational>)` / `ValueViewRef::Rational(&BigRational)`; `VAL_RATIONAL = 0x0E`.

- [ ] **Step 1: Write the failing test** (in `value.rs` tests)

```rust
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
```

- [ ] **Step 2: Add imports, tag, variant, constructor**

In `value.rs`: `use num_rational::BigRational;`. Add `const TAG_RATIONAL: u64 = 31;`. Add `Rational(Rc<BigRational>)` to `ValueView` and `Rational(&'a BigRational)` to `ValueViewRef`. Add constructor:

```rust
pub fn rational(r: BigRational) -> Value {
    if r.is_integer() {
        Value::from_bigint(r.to_integer())
    } else {
        Value::from_rc_ptr(TAG_RATIONAL, Rc::new(r))
    }
}
```

- [ ] **Step 3: Add the trait/dispatch arms** (mirroring Task 1.1's list, with `BigRational` payload):

- `view()`: `TAG_RATIONAL => ValueView::Rational(unsafe { self.get_rc::<BigRational>() }),`
- `view_ref()`: `TAG_RATIONAL => ValueViewRef::Rational(unsafe { self.borrow_ref::<BigRational>() }),`
- `heap_strong_count()`: `TAG_RATIONAL => count_at::<BigRational>(ptr),`
- `type_name()`: add a distinct arm `TAG_RATIONAL => "rational",`
- `Clone`: `TAG_RATIONAL => Rc::increment_strong_count(ptr as *const BigRational),`
- `Drop`: `TAG_RATIONAL => drop(Rc::from_raw(ptr as *const BigRational)),`
- `Display`: `ValueViewRef::Rational(r) => write!(f, "{}/{}", r.numer(), r.denom()),`
- `Debug`: `ValueView::Rational(r) => write!(f, "Rational({}/{})", r.numer(), r.denom()),`
- `PartialEq`: `(ValueViewRef::Rational(a), ValueViewRef::Rational(b)) => a == b,`
- `Hash`: `ValueViewRef::Rational(r) => { 31u8.hash(state); r.hash(state); }`
- `Ord`: in `type_order`, `ValueViewRef::Rational(_) => 18,` (its own bucket, consistent with the existing int/float bucketing); in the main match, `(ValueViewRef::Rational(a), ValueViewRef::Rational(b)) => a.cmp(b),`

Add accessors:

```rust
#[inline(always)]
pub fn is_rational(&self) -> bool {
    is_boxed(self.0) && get_tag(self.0) == TAG_RATIONAL
}
pub fn as_rational(&self) -> Option<BigRational> {
    match self.view_ref() {
        ValueViewRef::Int(n) => Some(BigRational::from(BigInt::from(n))),
        ValueViewRef::BigInt(n) => Some(BigRational::from((**n).clone())),
        ValueViewRef::Rational(r) => Some(r.clone()),
        _ => None,
    }
}
```

- [ ] **Step 4: Extend the tower bridge**

In `as_number`, add: `ValueViewRef::Rational(r) => Some(SemaNumber::Rational(r.clone())),`
In `from_number`, replace the `Rational` half of the `unreachable!` arm with: `SemaNumber::Rational(r) => Value::rational(r),` (leave `Complex` unreachable until Phase 3).

- [ ] **Step 5: Serialize `VAL_RATIONAL`**

`serialize.rs`: `const VAL_RATIONAL: u8 = 0x0E;`. In `serialize_value`, before the `_`:

```rust
ValueView::Rational(r) => {
    buf.push(VAL_RATIONAL);
    for part in [r.numer(), r.denom()] {
        let bytes = part.to_signed_bytes_le();
        let len = checked_u32(bytes.len(), "rational part byte length")?;
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&bytes);
    }
}
```

In `deserialize_value_inner`:

```rust
VAL_RATIONAL => {
    let read_part = |buf: &[u8], cursor: &mut usize| -> Result<num_bigint::BigInt, SemaError> {
        let len = read_u32_le(buf, cursor)? as usize;
        let bytes = read_bytes(buf, cursor, len)?;
        Ok(num_bigint::BigInt::from_signed_bytes_le(&bytes))
    };
    let numer = read_part(buf, cursor)?;
    let denom = read_part(buf, cursor)?;
    if denom == num_bigint::BigInt::from(0) {
        return Err(SemaError::eval("zero denominator in serialized rational"));
    }
    Ok(Value::rational(num_rational::BigRational::new(numer, denom)))
}
```

Add `num-rational` to `crates/sema-vm/Cargo.toml`. Document `0x0E VAL_RATIONAL` in the format spec.

- [ ] **Step 6: Build, test, smoke, commit**

Run: `cargo test -p sema-core value::tests::rational_roundtrip_and_normalize && cargo test -p sema-vm serialize:: && jake smoke-bytecode`
Expected: PASS.

```bash
git add crates/sema-core/src/value.rs crates/sema-vm/src/serialize.rs crates/sema-vm/Cargo.toml website/docs/internals/bytecode-format.md Cargo.lock
git commit -m "core+vm: add TAG_RATIONAL exact rationals with serialization"
```

### Task 2.2: Reader parses `1/3` rational literals

**Files:**
- Modify: `crates/sema-reader/src/lexer.rs`, `crates/sema-reader/src/reader.rs`
- Test: `lexer.rs` tests + `eval_test.rs`

**Interfaces:**
- Consumes: `SemaNumber::parse_rational`, `Value::rational`.
- Produces: `Token::Rational(BigRational)`; `1/3`, `-22/7`, `6/3` (→ int 2) read correctly. A lone `/` or `a/b` where `a`/`b` aren't integer digit-runs is still the symbol `/`.

`read_number` currently stops at `/`. Extend it: after lexing the integer body (no fraction, no exponent), if the next char is `/` followed by a digit, consume `/` + digits and emit a rational token.

- [ ] **Step 1: Write the failing test** (lexer)

```rust
#[test]
fn rational_literals() {
    use num_bigint::BigInt;
    use num_rational::BigRational;
    let first = |src: &str| tokenize(src).unwrap().into_iter().next().unwrap().token;
    assert_eq!(first("1/3"), Token::Rational(BigRational::new(BigInt::from(1), BigInt::from(3))));
    assert_eq!(first("-22/7"), Token::Rational(BigRational::new(BigInt::from(-22), BigInt::from(7))));
    // a lone slash is still the division symbol
    assert!(matches!(first("/"), Token::Symbol(s) if s == "/"));
    // 1.5/2 is NOT a rational (float numerator) — 1.5 then symbol
    assert_eq!(first("1.5"), Token::Float(1.5));
}
```

- [ ] **Step 2: Implement**

Add `Token::Rational(num_rational::BigRational)`. In `read_number`, in the integer branch (only when `!is_float`), before returning `Token::Int`/`Token::BigInt`, peek for a rational tail:

```rust
// Rational tail: `/` immediately followed by ≥1 digit (optionally after the
// integer body). `1/3`, `-22/7`. A `/` not followed by a digit is left to
// the symbol lexer (so `/` and `a/b`-symbols are unaffected).
if !is_float && i < chars.len() && chars[i] == '/' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
    let denom_start = i + 1;
    let mut j = denom_start;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    let numer_str: String = chars[..i].iter().collect();
    let denom_str: String = chars[denom_start..j].iter().collect();
    let numer = num_bigint::BigInt::parse_bytes(numer_str.as_bytes(), 10)
        .ok_or_else(|| SemaError::Reader { message: format!("invalid rational numerator: {numer_str}"), span: *span })?;
    let denom = num_bigint::BigInt::parse_bytes(denom_str.as_bytes(), 10)
        .ok_or_else(|| SemaError::Reader { message: format!("invalid rational denominator: {denom_str}"), span: *span })?;
    if denom == num_bigint::BigInt::from(0) {
        return Err(SemaError::Reader { message: "rational literal has zero denominator".into(), span: *span });
    }
    return Ok((Token::Rational(num_rational::BigRational::new(numer, denom)), j));
}
```

Add `num-rational` to `crates/sema-reader/Cargo.toml`. In `reader.rs`, add `Token::Rational(r) => Value::rational(r.clone()),` next to the `Token::BigInt` arm.

- [ ] **Step 3: Test + commit**

Run: `cargo test -p sema-reader rational_literals`
Expected: PASS.

```bash
git add crates/sema-reader/src/lexer.rs crates/sema-reader/src/reader.rs crates/sema-reader/Cargo.toml Cargo.lock
git commit -m "reader: parse 1/3 rational literals"
```

### Task 2.3: `/` returns exact rationals; arithmetic over rationals

**Files:**
- Modify: `crates/sema-stdlib/src/arithmetic.rs`, `crates/sema-vm/src/vm.rs`
- Test: `eval_test.rs`

**Interfaces:**
- Consumes: `SemaNumber::div`, `Value::as_number`, `Value::from_number`.
- Produces: `(/ 1 3)` → `1/3`; `(/ 6 3)` → `2`; `(/ 1 2.0)` → `0.5`; `(/ 1 0)` still errors "division by zero"; `(+ 1/2 1/3)` → `5/6`.

- [ ] **Step 1: Write the failing tests**

```rust
"(/ 1 3)" => common::eval_tw("1/3"),
"(/ 6 3)" => common::eval_tw("2"),
"(/ 10 4)" => common::eval_tw("5/2"),
"(/ 1 2.0)" => common::eval_tw("0.5"),
"(+ 1/2 1/3)" => common::eval_tw("5/6"),
"(* 2/3 3/4)" => common::eval_tw("1/2"),
"(- 1/2 1/3)" => common::eval_tw("1/6"),
"(/ 1 3 2)" => common::eval_tw("1/6"),
```

Add error test to `eval_error_tests!`: `"(/ 1 0)" => "division by zero"`.

- [ ] **Step 2: Rewrite `/` over the tower**

Replace the body of the `/` registration in `arithmetic.rs`. Fold left through `SemaNumber::div`, mapping `DivByZero` to the existing error+hint:

```rust
register_fn(env, "/", |args| {
    check_arity!(args, "/", 2..);
    let mut acc = args[0]
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    for arg in &args[1..] {
        let d = arg
            .as_number()
            .ok_or_else(|| SemaError::type_error("number", arg.type_name()))?;
        acc = acc.div(d).map_err(|_| {
            SemaError::eval("/: division by zero")
                .with_hint("/: guard with (if (zero? d) ... (/ n d))")
        })?;
    }
    Ok(Value::from_number(acc))
});
```

This removes the old exact-int fast path and the lossy `result as i64` tail — the tower handles exactness correctly. (Single-arg `/` reciprocal, `(/ 3)` → `1/3`, is R7RS; the current impl requires 2 args — keep `2..` unless adding reciprocal is desired. If adding: on `args.len()==1`, compute `SemaNumber::from_i64(1).div(acc)`.)

- [ ] **Step 3: Fix `vm_div`'s Phase-1 stopgap**

In `vm.rs`, replace the Phase-1 float stopgap in `vm_div`'s `_` arm with exact tower division:

```rust
_ => match (a.as_number(), b.as_number()) {
    (Some(x), Some(y)) => x.div(y)
        .map(Value::from_number)
        .map_err(|_| SemaError::eval("/: division by zero").with_hint("/: guard with (if (zero? d) ... (/ n d))")),
    _ => Err(SemaError::type_error("number", a.type_name())),
},
```

Also update the existing `(Int,Int)` exact arm of `vm_div` (line ~4398): currently it returns int when evenly divisible else float. Change the non-even case to exact rational so the VM matches the stdlib:

```rust
(ValueViewRef::Int(x), ValueViewRef::Int(y)) => {
    if y == 0 {
        Err(SemaError::eval("/: division by zero").with_hint("/: guard with (if (zero? d) ... (/ n d))"))
    } else if x % y == 0 {
        Ok(Value::int(x / y))
    } else {
        Ok(Value::from_number(SemaNumber::from_i64(x).div(SemaNumber::from_i64(y)).unwrap()))
    }
}
```

- [ ] **Step 4: Test + smoke + commit**

Run: `cargo test -p sema --test eval_test && jake smoke-bytecode`
Expected: PASS.

```bash
git add crates/sema-stdlib/src/arithmetic.rs crates/sema-vm/src/vm.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib+vm: / yields exact rationals; rational arithmetic via tower"
```

### Task 2.4: Rational predicates and accessors

**Files:**
- Modify: `crates/sema-stdlib/src/predicates.rs`, `crates/sema-stdlib/src/math.rs`
- Test: `eval_test.rs`

**Interfaces:**
- Produces builtins: `rational?`, `exact?`, `inexact?`, `exact-integer?`, `numerator`, `denominator`.

- [ ] **Step 1: Write the failing tests**

```rust
"(rational? 1/3)" => common::eval_tw("#t"),
"(rational? 5)" => common::eval_tw("#t"),
"(rational? 2.5)" => common::eval_tw("#f"),
"(exact? 1/3)" => common::eval_tw("#t"),
"(exact? 5)" => common::eval_tw("#t"),
"(exact? 2.5)" => common::eval_tw("#f"),
"(inexact? 2.5)" => common::eval_tw("#t"),
"(exact-integer? 5)" => common::eval_tw("#t"),
"(exact-integer? 1/3)" => common::eval_tw("#f"),
"(exact-integer? 2.0)" => common::eval_tw("#f"),
"(numerator 6/4)" => common::eval_tw("3"),
"(denominator 6/4)" => common::eval_tw("2"),
"(numerator 5)" => common::eval_tw("5"),
"(denominator 5)" => common::eval_tw("1"),
```

- [ ] **Step 2: Implement the predicates** (in `predicates.rs`)

```rust
register_fn(env, "rational?", |args| {
    check_arity!(args, "rational?", 1);
    // Every real number that is not an infinity/NaN is rational.
    let ok = match args[0].as_number() {
        Some(SemaNumber::Complex(_)) | None => false,
        Some(SemaNumber::Real(f)) => f.is_finite(),
        Some(_) => true,
    };
    Ok(Value::bool(ok))
});
register_fn(env, "exact?", |args| {
    check_arity!(args, "exact?", 1);
    Ok(Value::bool(args[0].as_number().map_or(false, |n| n.is_exact())))
});
register_fn(env, "inexact?", |args| {
    check_arity!(args, "inexact?", 1);
    Ok(Value::bool(args[0].as_number().map_or(false, |n| !n.is_exact())))
});
register_fn(env, "exact-integer?", |args| {
    check_arity!(args, "exact-integer?", 1);
    Ok(Value::bool(args[0].is_int() || args[0].is_bigint()))
});
```

Add `use sema_core::number::SemaNumber;` to `predicates.rs`.

- [ ] **Step 3: Implement `numerator`/`denominator`** (in `math.rs`)

```rust
register_fn(env, "numerator", |args| {
    check_arity!(args, "numerator", 1);
    match args[0].as_rational() {
        Some(r) => Ok(Value::from_bigint(r.numer().clone())),
        None => Err(SemaError::type_error("rational", args[0].type_name())),
    }
});
register_fn(env, "denominator", |args| {
    check_arity!(args, "denominator", 1);
    match args[0].as_rational() {
        Some(r) => Ok(Value::from_bigint(r.denom().clone())),
        None => Err(SemaError::type_error("rational", args[0].type_name())),
    }
});
```

- [ ] **Step 4: Test, lint, commit**

Run: `cargo test -p sema --test eval_test && jake lint`

```bash
git add crates/sema-stdlib/src/predicates.rs crates/sema-stdlib/src/math.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: rational?/exact?/inexact?/exact-integer?/numerator/denominator"
```

**Phase 2 exit criteria:** exact rational arithmetic (`+ - * /`) and comparison are correct through both the VM and stdlib; `1/3` reads/prints/serializes; `exact?`/`rational?`/`numerator`/`denominator` work. `jake test && jake smoke-bytecode && jake lint` green.

## Phase 3 — Complex numbers

After this phase, `(sqrt -1)` → `+i` (well, `0+1i`), `(make-rectangular 3 4)` → `3+4i`, `(* 3+4i 1-2i)` → `11-2i`, and complex literals read. Complex is a leaf heap type under `TAG_COMPLEX` carrying a `number::Complex`.

### Task 3.1: Structural `Eq`/`Hash`/`Ord` for `SemaNumber`, then `TAG_COMPLEX` wiring

**Files:**
- Modify: `crates/sema-core/src/number.rs`, `crates/sema-core/src/value.rs`, `crates/sema-vm/src/serialize.rs` + spec

**Interfaces:**
- Produces: `impl PartialEq/Eq/Hash/Ord for SemaNumber` and `Complex` (structural, for use as map-key components); `Value::complex(re: SemaNumber, im: SemaNumber) -> Value`; `Value::as_complex(&self) -> Option<Complex>`; `Value::is_complex`; `ValueView::Complex(Rc<Complex>)`/`ValueViewRef::Complex(&Complex)`; `VAL_COMPLEX = 0x0F`.

- [ ] **Step 1: Write the failing test** (number.rs — structural traits)

```rust
#[test]
fn structural_traits() {
    use std::collections::HashSet;
    let a = SemaNumber::Integer(BigInt::from(3));
    let b = SemaNumber::Integer(BigInt::from(3));
    assert_eq!(a, b);
    let mut set = HashSet::new();
    set.insert(SemaNumber::Real(1.5));
    assert!(set.contains(&SemaNumber::Real(1.5)));
    // Ordering by level then value (used only for deterministic map keys).
    assert!(SemaNumber::Integer(BigInt::from(1)) < SemaNumber::Real(0.0)); // level 0 < 2
}
```

- [ ] **Step 2: Implement structural traits in `number.rs`**

```rust
impl PartialEq for SemaNumber {
    fn eq(&self, other: &Self) -> bool {
        use SemaNumber::*;
        match (self, other) {
            (Integer(a), Integer(b)) => a == b,
            (Rational(a), Rational(b)) => a == b,
            (Real(a), Real(b)) => a.to_bits() == b.to_bits(),
            (Complex(a), Complex(b)) => a.re == b.re && a.im == b.im,
            _ => false,
        }
    }
}
impl Eq for SemaNumber {}
impl std::hash::Hash for SemaNumber {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use SemaNumber::*;
        match self {
            Integer(n) => { 0u8.hash(state); n.hash(state); }
            Rational(r) => { 1u8.hash(state); r.hash(state); }
            Real(f) => { 2u8.hash(state); f.to_bits().hash(state); }
            Complex(c) => { 3u8.hash(state); c.re.hash(state); c.im.hash(state); }
        }
    }
}
impl Ord for SemaNumber {
    fn cmp(&self, other: &Self) -> Ordering {
        use SemaNumber::*;
        self.level().cmp(&other.level()).then_with(|| match (self, other) {
            (Integer(a), Integer(b)) => a.cmp(b),
            (Rational(a), Rational(b)) => a.cmp(b),
            (Real(a), Real(b)) => a.total_cmp(b),
            (Complex(a), Complex(b)) => a.re.cmp(&b.re).then_with(|| a.im.cmp(&b.im)),
            _ => Ordering::Equal, // different levels already decided by level().cmp
        })
    }
}
impl PartialOrd for SemaNumber {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}
```

Run: `cargo test -p sema-core number::tests::structural_traits` → PASS.

- [ ] **Step 3: Add `TAG_COMPLEX` and trait arms in `value.rs`**

`const TAG_COMPLEX: u64 = 32;`. Add `Complex(Rc<SemaComplex>)` to `ValueView` and `Complex(&'a SemaComplex)` to `ValueViewRef` (recall `use crate::number::Complex as SemaComplex;` from Task 1.1). Constructor:

```rust
pub fn complex(re: SemaNumber, im: SemaNumber) -> Value {
    Value::from_number(SemaNumber::Complex(Box::new(SemaComplex { re, im })))
}
```

Arms (mirror Task 1.1, payload `SemaComplex`):
- `view()`: `TAG_COMPLEX => ValueView::Complex(unsafe { self.get_rc::<SemaComplex>() }),`
- `view_ref()`: `TAG_COMPLEX => ValueViewRef::Complex(unsafe { self.borrow_ref::<SemaComplex>() }),`
- `heap_strong_count()`: `TAG_COMPLEX => count_at::<SemaComplex>(ptr),`
- `type_name()`: `TAG_COMPLEX => "complex",`
- `Clone`: `TAG_COMPLEX => Rc::increment_strong_count(ptr as *const SemaComplex),`
- `Drop`: `TAG_COMPLEX => drop(Rc::from_raw(ptr as *const SemaComplex)),`
- `Display`: `ValueViewRef::Complex(c) => write!(f, "{}", SemaNumber::Complex(Box::new((*c).clone()))),` (reuses the tower's `Display`)
- `Debug`: `ValueView::Complex(c) => write!(f, "Complex({})", SemaNumber::Complex(Box::new((**c).clone()))),`
- `PartialEq`: `(ValueViewRef::Complex(a), ValueViewRef::Complex(b)) => a.re == b.re && a.im == b.im,`
- `Hash`: `ValueViewRef::Complex(c) => { 32u8.hash(state); c.re.hash(state); c.im.hash(state); }`
- `Ord`: `type_order` → `ValueViewRef::Complex(_) => 19,`; main match → `(ValueViewRef::Complex(a), ValueViewRef::Complex(b)) => a.re.cmp(&b.re).then_with(|| a.im.cmp(&b.im)),`

Accessors:

```rust
#[inline(always)]
pub fn is_complex(&self) -> bool {
    is_boxed(self.0) && get_tag(self.0) == TAG_COMPLEX
}
pub fn as_complex(&self) -> Option<SemaComplex> {
    if let ValueViewRef::Complex(c) = self.view_ref() { Some(c.clone()) } else { None }
}
```

Tower bridge: `as_number` gains `ValueViewRef::Complex(c) => Some(SemaNumber::Complex(Box::new(c.clone()))),`; `from_number`'s `Complex` arm becomes `SemaNumber::Complex(c) => Value::from_rc_ptr(TAG_COMPLEX, Rc::new(*c)),` (no more `unreachable!`).

- [ ] **Step 4: Serialize `VAL_COMPLEX`**

`serialize.rs`: `const VAL_COMPLEX: u8 = 0x0F;`. Serialize a complex as its two components, each a recursively-serialized `Value` (each component is Integer/Rational/Real, all already serializable):

```rust
ValueView::Complex(c) => {
    buf.push(VAL_COMPLEX);
    serialize_value(&Value::from_number(c.re.clone()), buf, stb)?;
    serialize_value(&Value::from_number(c.im.clone()), buf, stb)?;
}
```

Deserialize:

```rust
VAL_COMPLEX => {
    let re = deserialize_value_inner(buf, cursor, table, remap, depth + 1)?;
    let im = deserialize_value_inner(buf, cursor, table, remap, depth + 1)?;
    let (re, im) = (re.as_number().unwrap(), im.as_number().unwrap());
    Ok(Value::complex(re, im))
}
```

Document `0x0F VAL_COMPLEX` (two nested value constants: real then imaginary part).

- [ ] **Step 5: Build, test, smoke, commit**

Run: `cargo test -p sema-core && cargo test -p sema-vm serialize:: && jake smoke-bytecode`

```bash
git add crates/sema-core/src/number.rs crates/sema-core/src/value.rs crates/sema-vm/src/serialize.rs website/docs/internals/bytecode-format.md
git commit -m "core+vm: add TAG_COMPLEX complex numbers with structural traits + serialization"
```

### Task 3.2: Reader parses complex literals (`3+4i`, `-2i`, `+i`)

**Files:**
- Modify: `crates/sema-reader/src/lexer.rs`, `crates/sema-reader/src/reader.rs`
- Test: `lexer.rs` tests + `eval_test.rs`

**Interfaces:**
- Consumes: `SemaNumber`, `Value::complex`.
- Produces: `Token::Complex(SemaNumber /*re*/, SemaNumber /*im*/)`; forms `a+bi`, `a-bi`, `+bi`/`-bi`/`bi` (pure imaginary), `+i`/`-i`. Components may be int, rational, or float.

This is the fiddliest lexer change. A complex literal is `[real] (+|-) [ureal] i` or a pure-imaginary `(+|-)? ureal? i`. Implement by: when `read_number` finishes a real, if the next char begins a signed second number that ends in `i`, consume it as the imaginary part; also handle a token that is itself `<number>i` or `+i`/`-i`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn complex_literals() {
    use sema_core::number::SemaNumber;
    let n = |v: i64| SemaNumber::from_i64(v);
    let first = |src: &str| tokenize(src).unwrap().into_iter().next().unwrap().token;
    assert_eq!(first("3+4i"), Token::Complex(n(3), n(4)));
    assert_eq!(first("0-1i"), Token::Complex(n(0), n(-1)));
    assert_eq!(first("+i"),   Token::Complex(n(0), n(1)));
    assert_eq!(first("-i"),   Token::Complex(n(0), n(-1)));
    assert_eq!(first("2i"),   Token::Complex(n(0), n(2)));
    assert_eq!(first("1.5+2.5i"), Token::Complex(SemaNumber::Real(1.5), SemaNumber::Real(2.5)));
    // plain numbers unaffected
    assert_eq!(first("42"), Token::Int(42));
    // a symbol containing i is not a complex
    assert!(matches!(first("list"), Token::Symbol(s) if s == "list"));
}
```

- [ ] **Step 2: Implement**

Add `Token::Complex(num::SemaNumber, num::SemaNumber)` (re, im). Introduce a helper `parse_real_component(&str) -> Option<SemaNumber>` that reuses `SemaNumber::parse_int_radix(.,10)`/`parse_rational`/`f64::parse` to turn a numeric substring into a tower real. Then in the lexer's number entry point:

1. Lex the leading real substring exactly as today (tracking whether it was int/rational/float), but do NOT emit yet.
2. If the immediately following characters match an imaginary tail — `i` (bare, meaning the leading part was the imaginary magnitude → pure imaginary), or `(+|-) <ureal> i` — consume them and emit `Token::Complex`.
3. The special cases `+i` / `-i` (no digits) are recognized in the symbol-start path: when the lexer is about to read `+`/`-` and the next char is `i` and the char after `i` is a delimiter, emit `Token::Complex(0, ±1)`.

Because this interacts with the symbol lexer (`+`, `-`, and `i` are symbol-legal), gate complex recognition strictly: only treat a trailing `i` as imaginary when it is followed by a token delimiter (whitespace, `)`, `]`, `}`, `"`, EOF, `;`). This prevents `pi`, `list`, `imag` from being mis-lexed. Add a `is_delimiter(char)` check.

Concretely, extend `read_number` to return complex when a valid imaginary tail with delimiter follows, and add a small branch in the top-level tokenizer for the bare `+i`/`-i` case. Full code (structured to keep the existing int/rational/float returns intact):

```rust
// After computing the leading real `s`/`is_float` in read_number and BEFORE the
// integer/float return, check for a complex form.
// Case A: leading real is the imaginary magnitude: `<real>i<delim>`.
if i < chars.len() && chars[i] == 'i' && is_delimiter_at(chars, i + 1) {
    let im = parse_real_component(&s, is_float, span)?;
    return Ok((Token::Complex(SemaNumber::from_i64(0), im), i + 1));
}
// Case B: `<real>(+|-)<ureal>i<delim>`.
if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
    if let Some((im, consumed)) = try_imaginary_tail(chars, i, span)? {
        let re = parse_real_component(&s, is_float, span)?;
        return Ok((Token::Complex(re, im), consumed));
    }
}
```

where `try_imaginary_tail` lexes an optional sign + ureal (int/rational/float, or empty meaning ±1) followed by `i` + delimiter, returning the imaginary `SemaNumber` and the absolute end index. `is_delimiter_at` returns true at EOF or on a delimiter char. The bare `+i`/`-i` (no leading real) case is handled by calling `try_imaginary_tail(chars, 0, span)` from the symbol lexer when it encounters a leading `+`/`-`; if it matches, emit `Token::Complex(0, im)`.

(Subagent guidance: write `try_imaginary_tail` and `parse_real_component` as standalone fns with their own unit tests before wiring them into `read_number`/the tokenizer. This isolates the tricky delimiter logic.)

In `reader.rs`: `Token::Complex(re, im) => Value::complex(re.clone(), im.clone()),`.

- [ ] **Step 3: Test + commit**

Run: `cargo test -p sema-reader complex_literals`

```bash
git add crates/sema-reader/src/lexer.rs crates/sema-reader/src/reader.rs crates/sema/tests/eval_test.rs
git commit -m "reader: parse complex literals (3+4i, +i, 2i)"
```

### Task 3.3: Complex builtins and complex-producing operations

**Files:**
- Modify: `crates/sema-stdlib/src/math.rs`, `crates/sema-stdlib/src/predicates.rs`
- Test: `eval_test.rs`

**Interfaces:**
- Produces builtins: `make-rectangular`, `make-polar`, `real-part`, `imag-part`, `magnitude`, `angle`, and predicates `complex?`, `real?`. Plus: `sqrt` of a negative real returns complex; arithmetic over complex already works via the tower (Task 3.1).

- [ ] **Step 1: Write the failing tests**

```rust
"(make-rectangular 3 4)" => common::eval_tw("3+4i"),
"(real-part 3+4i)" => common::eval_tw("3"),
"(imag-part 3+4i)" => common::eval_tw("4"),
"(real-part 5)" => common::eval_tw("5"),
"(imag-part 5)" => common::eval_tw("0"),
"(magnitude 3+4i)" => common::eval_tw("5.0"),
"(* 3+4i 1-2i)" => common::eval_tw("11-2i"),
"(+ 1+2i 3+4i)" => common::eval_tw("4+6i"),
"(complex? 3+4i)" => common::eval_tw("#t"),
"(complex? 5)" => common::eval_tw("#t"),
"(real? 3+4i)" => common::eval_tw("#f"),
"(real? 5)" => common::eval_tw("#t"),
"(sqrt -1)" => common::eval_tw("0+1i"),
"(sqrt -4)" => common::eval_tw("0+2i"),
```

Note the R7RS subtlety: `complex?` is true for ALL numbers (every real is complex); `real?` is true for non-complex. `magnitude` of an exact `3+4i` is `5.0` (inexact, via `sqrt`).

- [ ] **Step 2: Implement the builtins** (`math.rs`)

```rust
register_fn(env, "make-rectangular", |args| {
    check_arity!(args, "make-rectangular", 2);
    let re = args[0].as_number().filter(|n| n.is_real())
        .ok_or_else(|| SemaError::type_error("real", args[0].type_name()))?;
    let im = args[1].as_number().filter(|n| n.is_real())
        .ok_or_else(|| SemaError::type_error("real", args[1].type_name()))?;
    Ok(Value::complex(re, im))
});
register_fn(env, "make-polar", |args| {
    check_arity!(args, "make-polar", 2);
    let m = args[0].as_number().map(|n| n.to_f64())
        .ok_or_else(|| SemaError::type_error("real", args[0].type_name()))?;
    let a = args[1].as_number().map(|n| n.to_f64())
        .ok_or_else(|| SemaError::type_error("real", args[1].type_name()))?;
    Ok(Value::complex(SemaNumber::Real(m * a.cos()), SemaNumber::Real(m * a.sin())))
});
register_fn(env, "real-part", |args| {
    check_arity!(args, "real-part", 1);
    match args[0].as_number() {
        Some(SemaNumber::Complex(c)) => Ok(Value::from_number(c.re)),
        Some(real) => Ok(Value::from_number(real)),
        None => Err(SemaError::type_error("number", args[0].type_name())),
    }
});
register_fn(env, "imag-part", |args| {
    check_arity!(args, "imag-part", 1);
    match args[0].as_number() {
        Some(SemaNumber::Complex(c)) => Ok(Value::from_number(c.im)),
        Some(_) => Ok(Value::int(0)), // exact 0 imaginary for a real
        None => Err(SemaError::type_error("number", args[0].type_name())),
    }
});
register_fn(env, "magnitude", |args| {
    check_arity!(args, "magnitude", 1);
    match args[0].as_number() {
        Some(SemaNumber::Complex(c)) => {
            let (re, im) = (c.re.to_f64(), c.im.to_f64());
            Ok(Value::float(re.hypot(im)))
        }
        Some(real) => Ok(Value::from_number(real.abs())), // abs added in Phase 5; see note
        None => Err(SemaError::type_error("number", args[0].type_name())),
    }
});
register_fn(env, "angle", |args| {
    check_arity!(args, "angle", 1);
    match args[0].as_number() {
        Some(SemaNumber::Complex(c)) => Ok(Value::float(c.im.to_f64().atan2(c.re.to_f64()))),
        Some(real) => Ok(Value::float(if real.to_f64() < 0.0 { std::f64::consts::PI } else { 0.0 })),
        None => Err(SemaError::type_error("number", args[0].type_name())),
    }
});
```

`magnitude`'s real branch calls `SemaNumber::abs` — add that method to `number.rs` now (it is also used by Phase 5's `abs`): `pub fn abs(self) -> SemaNumber` returning the non-negative magnitude for real inputs (Integer/Rational/Real) and — for a Complex — its `f64` hypot wrapped as `Real`. Add a `number.rs` unit test for it.

- [ ] **Step 3: `complex?`/`real?` predicates** (`predicates.rs`)

```rust
register_fn(env, "complex?", |args| {
    check_arity!(args, "complex?", 1);
    Ok(Value::bool(args[0].as_number().is_some())) // every number is complex
});
register_fn(env, "real?", |args| {
    check_arity!(args, "real?", 1);
    Ok(Value::bool(args[0].as_number().map_or(false, |n| n.is_real())))
});
```

- [ ] **Step 4: `sqrt` returns complex for negative reals** (`math.rs`)

Find the existing `sqrt` registration. Wrap its logic: if the argument is a real `< 0`, return `Value::complex(0, sqrt(|x|))` (inexact); if it is already complex, use the complex square root formula; otherwise the existing `f64::sqrt`. Also make exact perfect squares stay exact where cheap (optional — R7RS allows `(sqrt 4)` → `2` exact; implement via `num_integer::Roots::sqrt` on a non-negative `BigInt` when the arg is an exact integer and the root is exact, else fall to `f64`). Minimum for the tests: negative-real → complex.

```rust
register_fn(env, "sqrt", |args| {
    check_arity!(args, "sqrt", 1);
    let n = args[0].as_number().ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    match n {
        SemaNumber::Complex(c) => {
            // principal complex sqrt via polar form
            let (re, im) = (c.re.to_f64(), c.im.to_f64());
            let r = re.hypot(im).sqrt();
            let theta = im.atan2(re) / 2.0;
            Ok(Value::complex(SemaNumber::Real(r * theta.cos()), SemaNumber::Real(r * theta.sin())))
        }
        real if real.cmp_real(&SemaNumber::from_i64(0)) == Some(std::cmp::Ordering::Less) => {
            let mag = (-real.to_f64()).sqrt();
            Ok(Value::complex(SemaNumber::from_i64(0), SemaNumber::Real(mag)))
        }
        // exact perfect-square fast path
        SemaNumber::Integer(ref b) if b.sqrt().pow(2) == *b => {
            Ok(Value::from_bigint(b.sqrt()))
        }
        real => Ok(Value::float(real.to_f64().sqrt())),
    }
});
```

(`BigInt::sqrt` needs `use num_integer::Roots;`.)

- [ ] **Step 5: Test, lint, smoke, commit**

Run: `cargo test -p sema --test eval_test && jake lint && jake smoke-bytecode`

```bash
git add crates/sema-stdlib/src/math.rs crates/sema-stdlib/src/predicates.rs crates/sema-core/src/number.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: complex builtins (make-rectangular/real-part/magnitude/…), complex? real?, sqrt→complex"
```

**Phase 3 exit criteria:** complex construction, arithmetic, accessors, predicates, and `(sqrt -1)` all correct; complex literals read/print/serialize/round-trip; `jake test && jake smoke-bytecode && jake lint` green. The four-level tower (integer → rational → real → complex) is functionally complete.

## Phase 4 — Reader completeness (radix and exactness prefixes)

R7RS number prefixes: `#x`/`#o`/`#b`/`#d` (radix) and `#e`/`#i` (exactness), combinable (`#x#e1F`, `#e#xFF`). After this phase the reader accepts the full literal grammar.

### Task 4.1: Radix prefixes `#x #o #b #d`

**Files:**
- Modify: `crates/sema-reader/src/lexer.rs`
- Test: `lexer.rs` tests + `eval_test.rs`

**Interfaces:**
- Consumes: `SemaNumber::parse_int_radix`.
- Produces: `#xFF` → 255, `#b101` → 5, `#o17` → 15, `#d10` → 10, sign-aware (`#x-1F`), bignum-capable.

The `#` dispatch already exists in the lexer for `#t`/`#f`/`#\`/`#"`/`#(`/`#u8`. Add radix cases: on `#` followed by `x`/`X`/`o`/`O`/`b`/`B`/`d`/`D`, read the following digit-run (plus optional sign) in that radix via `parse_int_radix`, emit `Token::Int` (if it fits i64) or `Token::BigInt`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn radix_prefixes() {
    let first = |src: &str| tokenize(src).unwrap().into_iter().next().unwrap().token;
    assert_eq!(first("#xFF"), Token::Int(255));
    assert_eq!(first("#b101"), Token::Int(5));
    assert_eq!(first("#o17"), Token::Int(15));
    assert_eq!(first("#d10"), Token::Int(10));
    assert_eq!(first("#x-1F"), Token::Int(-31));
    assert_eq!(first("#xff"), Token::Int(255)); // lowercase digits
}
```

- [ ] **Step 2: Implement** — in the `#`-dispatch, add arms for the radix letters. Read the sign+digit run into a `String`, call `SemaNumber::parse_int_radix(&digits, radix)`, then convert: if the resulting `SemaNumber::Integer` fits i64 → `Token::Int`, else `Token::BigInt`. Emit a reader error on invalid digits. Return the exact consumed length (prefix `2` + sign + digits).

- [ ] **Step 3: Test + commit**

Run: `cargo test -p sema-reader radix_prefixes`
```bash
git add crates/sema-reader/src/lexer.rs crates/sema/tests/eval_test.rs
git commit -m "reader: radix prefixes #x #o #b #d"
```

### Task 4.2: Exactness prefixes `#e #i` and `exact`/`inexact`/conversions

**Files:**
- Modify: `crates/sema-reader/src/lexer.rs`, `crates/sema-stdlib/src/math.rs`
- Test: `lexer.rs` tests + `eval_test.rs`

**Interfaces:**
- Consumes: `SemaNumber::to_exact`/`to_inexact`, `Value::from_number`, `Value::as_number`.
- Produces: reader `#e1.5` → `3/2`, `#i1/2` → `0.5`, combinable with radix (`#e#xFF`, `#x#e1F`); builtins `exact`, `inexact`, `exact->inexact`, `inexact->exact`.

- [ ] **Step 1: Write the failing tests** (lexer + eval)

```rust
// lexer
#[test]
fn exactness_prefixes() {
    let first = |src: &str| tokenize(src).unwrap().into_iter().next().unwrap().token;
    // #i makes an exact literal inexact
    assert_eq!(first("#i1/2"), Token::Float(0.5));
    assert_eq!(first("#e1.5"), Token::Rational(num_rational::BigRational::new(3.into(), 2.into())));
}
```
```rust
// eval_test
"(exact->inexact 1/2)" => common::eval_tw("0.5"),
"(inexact->exact 0.5)" => common::eval_tw("1/2"),
"(exact 2.0)" => common::eval_tw("2"),
"(inexact 3)" => common::eval_tw("3.0"),
"#e1.5" => common::eval_tw("3/2"),
"#i1/2" => common::eval_tw("0.5"),
```

- [ ] **Step 2: Implement the prefix in the lexer** — `#e`/`#i` set a pending exactness flag, then recurse into the following number literal (which may itself carry a radix prefix). After lexing the inner number token, apply the flag: `#e` → convert `Token::Float`/`Token::Rational`/`Token::Int` to its exact form (`Float(f)` → the `Token::Rational`/`Token::Int` of `SemaNumber::Real(f).to_exact()`); `#i` → convert to `Token::Float`. Implement by lexing the remainder into a `SemaNumber`, applying `to_exact`/`to_inexact`, and emitting the matching token via a small `sema_number_to_token(SemaNumber)` helper. Combinable prefixes: allow `#e#x…`/`#x#e…` by looping the `#`-prefix reader until a digit/sign begins.

- [ ] **Step 3: Implement the builtins** (`math.rs`)

```rust
fn to_inexact_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "inexact", 1);
    let n = args[0].as_number().ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    Ok(Value::from_number(n.to_inexact()))
}
fn to_exact_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "exact", 1);
    let n = args[0].as_number().ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    Ok(Value::from_number(n.to_exact()))
}
register_fn(env, "inexact", to_inexact_impl);
register_fn(env, "exact->inexact", to_inexact_impl);
register_fn(env, "exact", to_exact_impl);
register_fn(env, "inexact->exact", to_exact_impl);
```

- [ ] **Step 4: Test, lint, commit**

Run: `cargo test -p sema-reader exactness_prefixes && cargo test -p sema --test eval_test && jake lint`
```bash
git add crates/sema-reader/src/lexer.rs crates/sema-stdlib/src/math.rs crates/sema/tests/eval_test.rs
git commit -m "reader+stdlib: #e/#i exactness prefixes and exact/inexact conversions"
```

**Phase 4 exit criteria:** the full R7RS numeric literal grammar reads correctly, including combined radix+exactness prefixes.

## Phase 5 — Stdlib math sweep (every numeric builtin over the tower)

Generalize the remaining `math.rs`, `comparison.rs`, and `bitwise.rs` builtins from `i64`/`f64`-only to the full tower. Each task is one family, TDD'd against `eval_test.rs`. The pattern is uniform: lift via `as_number`, compute in the tower (add tower methods to `number.rs` as needed), lower via `from_number`.

### Task 5.1: Comparison and sign predicates over the tower

**Files:** `crates/sema-stdlib/src/comparison.rs`, `eval_test.rs`

**Interfaces:** `< > <= >= =` and `zero? positive? negative? even? odd?` handle bignum/rational/complex operands (complex is only valid for `=`/`zero?`; ordering complex errors).

- [ ] **Step 1: Failing tests**

```rust
"(< 1/3 1/2)" => common::eval_tw("#t"),
"(< 1/2 0.6)" => common::eval_tw("#t"),
"(> 170141183460469231731687303715884105728 9223372036854775807)" => common::eval_tw("#t"),
"(= 1/2 0.5)" => common::eval_tw("#t"),
"(= 2 2.0 4/2)" => common::eval_tw("#t"),
"(zero? 0/5)" => common::eval_tw("#t"),
"(positive? 1/3)" => common::eval_tw("#t"),
"(negative? -1/3)" => common::eval_tw("#t"),
"(even? 170141183460469231731687303715884105728)" => common::eval_tw("#t"),
"(odd? 170141183460469231731687303715884105729)" => common::eval_tw("#t"),
```
Error tests: `"(< 1+2i 3)" => "cannot order complex"`.

- [ ] **Step 2: Implement** — rewrite `comparison.rs`'s `compare_two` helper (line ~10) to lift both operands via `as_number` and use `SemaNumber::cmp_real`, returning an error when it yields `None` due to a complex operand (distinguish NaN — which returns "false"/unordered — from complex, which is a type error; `cmp_real` returns `None` for both, so pre-check `is_real()`). Rewrite `=` to use `SemaNumber::num_eq` across all args. Rewrite `zero?/positive?/negative?` via `as_number` + `cmp_real` against zero. `even?/odd?` operate on `as_bigint` (error on non-integer), using `BigInt` parity (`(&n % 2u8).is_zero()`).

- [ ] **Step 3: Update `vm_lt`** to match (it already got a tower fallthrough in Task 1.5; extend it to error on complex rather than silently comparing). Test, commit.

Run: `cargo test -p sema --test eval_test && jake smoke-bytecode`
```bash
git add crates/sema-stdlib/src/comparison.rs crates/sema-vm/src/vm.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib+vm: comparison and sign predicates over the full tower"
```

### Task 5.2: Rounding — `floor ceiling round truncate` over rationals/bignums

**Files:** `crates/sema-core/src/number.rs`, `crates/sema-stdlib/src/math.rs`, `eval_test.rs`

**Interfaces:** add `SemaNumber::{floor,ceil,round,truncate}(self) -> SemaNumber` (exact-preserving: rounding a rational yields an exact integer; rounding a float yields a float; complex errors at the builtin layer). R7RS `round` uses banker's rounding (round-half-to-even).

- [ ] **Step 1: Failing tests**

```rust
"(floor 7/2)" => common::eval_tw("3"),
"(ceiling 7/2)" => common::eval_tw("4"),
"(round 7/2)" => common::eval_tw("4"),   // 3.5 → 4 (even)
"(round 5/2)" => common::eval_tw("2"),   // 2.5 → 2 (even) banker's rounding
"(truncate -7/2)" => common::eval_tw("-3"),
"(floor 2.5)" => common::eval_tw("2.0"),
"(floor 5)" => common::eval_tw("5"),
```

- [ ] **Step 2: Implement** the four methods in `number.rs` (using `num_integer::Integer::div_floor`/`div_ceil` on `numer`/`denom` for rationals; `f64::floor`/`ceil`/`round_ties_even`/`trunc` for reals; identity for integers), with unit tests. Then update the `math.rs` builtins to route real/rational/integer args through them (preserve any existing `math/round-to`, `math/format-fixed` behavior). Complex → type error.

- [ ] **Step 3: Test, commit.**
```bash
git add crates/sema-core/src/number.rs crates/sema-stdlib/src/math.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: floor/ceiling/round/truncate over rationals and bignums (banker's rounding)"
```

### Task 5.3: `abs min max` over the tower

**Files:** `crates/sema-stdlib/src/math.rs`, `crates/sema-core/src/number.rs`, `eval_test.rs`

**Interfaces:** `abs` (already added `SemaNumber::abs` in Task 3.3 — verify it handles rational/bignum), `min`/`max` compare via `cmp_real` and preserve exactness, with R7RS inexactness contagion (if any arg is inexact, the result is inexact even if the extremum was exact).

- [ ] **Step 1: Failing tests**
```rust
"(abs -1/3)" => common::eval_tw("1/3"),
"(abs -170141183460469231731687303715884105728)" => common::eval_tw("170141183460469231731687303715884105728"),
"(min 1/2 1/3 0.4)" => common::eval_tw("0.3333333333333333"),  // contagion → inexact
"(max 1/2 1/3)" => common::eval_tw("1/2"),
```
- [ ] **Step 2: Implement** `min`/`max` folding with `cmp_real`; track whether any operand was inexact and apply `to_inexact` to the winner if so. Route `abs` through `SemaNumber::abs` for all reals; complex `abs` = `magnitude`.
- [ ] **Step 3: Test, commit.**
```bash
git add crates/sema-stdlib/src/math.rs crates/sema-core/src/number.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: abs/min/max over the tower with inexactness contagion"
```

### Task 5.4: Integer division family over bignums — `quotient remainder modulo mod gcd lcm`

**Files:** `crates/sema-stdlib/src/math.rs`, `crates/sema-stdlib/src/arithmetic.rs`, `eval_test.rs`

**Interfaces:** all operate on `as_bigint` (error on non-integer), using `num_integer` (`div_rem`, `gcd`, `lcm`) and returning via `Value::from_bigint`. `modulo`/`mod` uses floored division (sign of divisor); `remainder`/`quotient` uses truncated division (sign of dividend), per R7RS.

- [ ] **Step 1: Failing tests**
```rust
"(quotient 100000000000000000000 7)" => common::eval_tw("14285714285714285714"),
"(remainder 100000000000000000000 7)" => common::eval_tw("2"),
"(modulo -7 3)" => common::eval_tw("2"),
"(remainder -7 3)" => common::eval_tw("-1"),
"(gcd 12 18)" => common::eval_tw("6"),
"(gcd 100000000000000000000 10)" => common::eval_tw("10"),
"(lcm 4 6)" => common::eval_tw("12"),
"(mod 10 3)" => common::eval_tw("1"),
```
- [ ] **Step 2: Implement** each via `as_bigint` + `num_integer`. `modulo`: `n.mod_floor(&d)`; `remainder`: `n % d` (truncated); `quotient`: `n / d` (truncated). Guard zero divisor with the existing error+hint. Replace `arithmetic.rs`'s `mod_impl` with a bignum-aware version (keep the float `%` behavior for float operands). `gcd`/`lcm` are variadic folds.
- [ ] **Step 3: Test, commit.**
```bash
git add crates/sema-stdlib/src/math.rs crates/sema-stdlib/src/arithmetic.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: quotient/remainder/modulo/gcd/lcm over bignums (R7RS division semantics)"
```

### Task 5.5: `expt` exact, plus transcendental functions

**Files:** `crates/sema-stdlib/src/math.rs`, `crates/sema-core/src/number.rs`, `eval_test.rs`

**Interfaces:** `expt`/`pow` with an exact base and non-negative exact integer exponent → exact (bignum/rational via repeated squaring); negative integer exponent of an exact base → exact rational; non-integer/float exponent → `f64::powf`. Transcendentals (`sin cos tan exp log log10 log2 asin acos atan atan2 sinh cosh tanh`) project to `f64` (inexact), unchanged except they accept exact args via `to_f64`.

- [ ] **Step 1: Failing tests**
```rust
"(expt 2 100)" => common::eval_tw("1267650600228229401496703205376"),
"(expt 2 -3)" => common::eval_tw("1/8"),
"(expt 1/2 3)" => common::eval_tw("1/8"),
"(expt 2.0 10)" => common::eval_tw("1024.0"),
"(expt 2 0.5)" => common::eval_tw("1.4142135623730951"),
```
- [ ] **Step 2: Implement** `SemaNumber::powi(self, exp: &BigInt) -> Result<SemaNumber, DivByZero>` in `number.rs` (repeated squaring on Integer/Rational; negative exp → reciprocal; base 0 with negative exp → err). In `math.rs`'s `expt`/`pow`/`math/pow`: if both args are exact and the exponent is an exact integer, use `powi`; else `Value::float(base.to_f64().powf(exp.to_f64()))`. Update transcendentals to accept any real via `as_number().to_f64()` (they already largely do; ensure bignum/rational args work).
- [ ] **Step 3: Test, commit.**
```bash
git add crates/sema-stdlib/src/math.rs crates/sema-core/src/number.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: expt exact for integer exponents; transcendentals accept exact args"
```

### Task 5.6: `number->string` / `string->number` with radix

**Files:** `crates/sema-stdlib/src/math.rs` (or `string.rs` where number/string conversions live), `eval_test.rs`

**Interfaces:** `(number->string n [radix])` and `(string->number s [radix])` — radix ∈ {2,8,10,16}. `string->number` returns `#f` on unparseable input (R7RS), and parses integers, bignums, rationals, floats, and (radix 10) complex.

- [ ] **Step 1: Failing tests**
```rust
"(number->string 255 16)" => common::eval_tw("\"ff\""),
"(number->string 1/3)" => common::eval_tw("\"1/3\""),
"(number->string 255)" => common::eval_tw("\"255\""),
"(string->number \"ff\" 16)" => common::eval_tw("255"),
"(string->number \"1/3\")" => common::eval_tw("1/3"),
"(string->number \"3.14\")" => common::eval_tw("3.14"),
"(string->number \"nope\")" => common::eval_tw("#f"),
"(string->number \"3+4i\")" => common::eval_tw("3+4i"),
```
- [ ] **Step 2: Implement** `number->string` via the tower `Display` for radix 10; for other radices, require an exact integer and use `BigInt::to_str_radix`. `string->number` delegates to a shared reader entry — reuse the lexer's number path: tokenize the string and accept it only if it yields exactly one numeric token; radix != 10 wraps the digits through `SemaNumber::parse_int_radix`. Return `Value::bool(false)` on any failure (never error).
- [ ] **Step 3: Test, commit.**
```bash
git add crates/sema-stdlib/src/math.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: number->string/string->number with radix over the tower"
```

### Task 5.7: `exact-integer-sqrt` and `rationalize`

**Files:** `crates/sema-stdlib/src/math.rs`, `crates/sema-core/src/number.rs`, `eval_test.rs`

**Interfaces:** `(exact-integer-sqrt n)` → two values `(s r)` with `s² + r = n`, `0 ≤ r`, exact (return a 2-element list). `(rationalize x tol)` → the simplest rational within `tol` of `x`.

- [ ] **Step 1: Failing tests**
```rust
"(exact-integer-sqrt 17)" => common::eval_tw("(4 1)"),
"(exact-integer-sqrt 100000000000000000000)" => common::eval_tw("(10000000000 0)"),
"(rationalize 1/3 1/100)" => common::eval_tw("1/3"),
"(rationalize 3.14159 1/100)" => common::eval_tw("22/7"),  // approx; adjust to actual simplest
```
(Verify the `rationalize` expected against the implemented Stern–Brocot result and pin the literal.)
- [ ] **Step 2: Implement** `exact-integer-sqrt` via `num_integer::Roots::sqrt` on a non-negative `BigInt` (error on negative or non-integer). `rationalize` via a Stern–Brocot / continued-fraction "simplest rational in interval" over `BigRational`. Add `SemaNumber` helpers + unit tests.
- [ ] **Step 3: Test, commit.**
```bash
git add crates/sema-stdlib/src/math.rs crates/sema-core/src/number.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: exact-integer-sqrt and rationalize"
```

### Task 5.8: Bitwise ops are bignum-aware

**Files:** `crates/sema-stdlib/src/bitwise.rs`, `eval_test.rs`

**Interfaces:** `bit/and bit/or bit/xor bit/not bit/shift-left bit/shift-right` accept bignums (two's-complement semantics via `BigInt`), returning bignums; still error on non-integers.

- [ ] **Step 1: Failing tests**
```rust
"(bit/shift-left 1 100)" => common::eval_tw("1267650600228229401496703205376"),
"(bit/and 170141183460469231731687303715884105727 255)" => common::eval_tw("255"),
"(bit/or 1152921504606846976 1)" => common::eval_tw("1152921504606846977"),
```
- [ ] **Step 2: Implement** — lift operands via `as_bigint`, use `BigInt`'s `&`/`|`/`^`/`!`/`<<`/`>>`, lower via `from_bigint`. Keep the i64 fast path for in-range operands if desired (measure; correctness first). Shift counts come from `as_index`.
- [ ] **Step 3: Test, lint, commit.**
```bash
git add crates/sema-stdlib/src/bitwise.rs crates/sema/tests/eval_test.rs
git commit -m "stdlib: bitwise ops bignum-aware (BigInt two's-complement)"
```

**Phase 5 exit criteria:** every numeric builtin operates correctly across the whole tower; `jake test && jake smoke-bytecode && jake lint` green.

## Phase 6 — Cross-cutting integration (JSON, fuzzer, docs)

### Task 6.1: JSON encoding/decoding of tower numbers

**Files:** `crates/sema-core/src/json.rs`, `crates/sema-stdlib/src/json.rs`, `eval_test.rs`

**Interfaces:** `json/encode` of a bignum emits its decimal integer digits (JSON permits arbitrary-precision integer syntax); a rational/complex has no JSON number form — emit a string (`"1/3"`, `"3+4i"`) so encoding never fails; `json/decode` of an integer beyond i64 yields a bignum.

- [ ] **Step 1: Failing tests**
```rust
"(json/encode 170141183460469231731687303715884105728)" => common::eval_tw("\"170141183460469231731687303715884105728\""),
"(json/encode 1/3)" => common::eval_tw("\"\\\"1/3\\\"\""),  // rational → JSON string "1/3"
"(json/decode \"170141183460469231731687303715884105728\")" => common::eval_tw("170141183460469231731687303715884105728"),
```
(Confirm exact expected strings against the JSON encoder's quoting and pin them.)
- [ ] **Step 2: Implement** — add `ValueView::BigInt`/`Rational`/`Complex` arms to both JSON encoders (bignum → raw digits; rational/complex → quoted `to_string()`). In the decoder, when a JSON integer literal overflows `i64`, parse it via `SemaNumber::parse_int_radix(.,10)` into a bignum.
- [ ] **Step 3: Test, commit.**
```bash
git add crates/sema-core/src/json.rs crates/sema-stdlib/src/json.rs crates/sema/tests/eval_test.rs
git commit -m "json: encode/decode bignums; rationals/complex as strings"
```

### Task 6.2: Grammar fuzzer round-trips the new literal forms

**Files:** `fuzz/grammar-fuzz.sema`

**Interfaces:** the printer↔reader round-trip oracle and the compiler/VM value oracle must cover bignum, rational, and complex literals so a regression in Display/reader/serialize is caught.

- [ ] **Step 1:** Add generators for the new literal forms to `grammar-fuzz.sema` (emit random bignums beyond i64, rationals `a/b`, and complex `a+bi`), so `jake fuzz.grammar-emit` produces them and the round-trip oracle exercises them.
- [ ] **Step 2:** Run: `jake fuzz.grammar` for a bounded number of seeds (e.g. `scripts/grammar-fuzz.sh` with a fixed seed range). Expected: no round-trip or value-oracle mismatches.
- [ ] **Step 3: Commit.**
```bash
git add fuzz/grammar-fuzz.sema
git commit -m "fuzz: round-trip bignum/rational/complex literals in the grammar fuzzer"
```

### Task 6.3: Documentation — builtin docs, limitations, ADR

**Files:** `crates/sema-docs/entries/*.md`, `docs/limitations.md`, `docs/adr.md`

- [ ] **Step 1:** Add one `crates/sema-docs/entries/<name>.md` per new builtin (`exact.md`, `inexact.md`, `exact->inexact.md`, `inexact->exact.md`, `rational.md` [predicate `rational?` — match the existing entry naming], `exact-integer.md`, `numerator.md`, `denominator.md`, `make-rectangular.md`, `make-polar.md`, `real-part.md`, `imag-part.md`, `magnitude.md`, `angle.md`, `complex.md`, `real.md`, `exact-integer-sqrt.md`, `rationalize.md`, `number-string.md`, `string-number.md`), following the format of an existing entry. Run `sema-docs gen` to regenerate the index.
- [ ] **Step 2:** In `docs/limitations.md`, delete the "No Full Numeric Tower — Only `i64` and `f64`" entry (line ~85) and the impact-table row (~221). If a "known limitations" summary elsewhere references it, update it.
- [ ] **Step 3:** Add an ADR entry in `docs/adr.md` documenting the numeric-tower design: the `SemaNumber` currency type, the three leaf tags, exactness contagion, and the "VM fast path + tower fallthrough" strategy.
- [ ] **Step 4: Commit.**
```bash
git add crates/sema-docs/entries docs/limitations.md docs/adr.md
git commit -m "docs: numeric-tower builtin docs, remove tower limitation, add ADR"
```

### Task 6.4: Document that typed arrays stay `i64`/`f64` (deliberate)

**Files:** `docs/deferred.md` (or a comment where `f64-array`/`i64-array` are defined)

- [ ] **Step 1:** Add a short `docs/deferred.md` note: `TAG_F64_ARRAY`/`TAG_I64_ARRAY` typed arrays remain fixed-width `f64`/`i64` by design (they are performance containers); storing a bignum/rational into one narrows it or errors, exactly as today. This is intentional, not a tower gap.
- [ ] **Step 2: Commit.**
```bash
git add docs/deferred.md
git commit -m "docs: note typed arrays stay fixed-width by design (not a tower gap)"
```

## Phase 7 — Verification and hardening

### Task 7.1: Property tests — tower algebraic laws and VM/stdlib parity

**Files:** `crates/sema-core/tests/number_props.rs` (new), `crates/sema/tests/eval_test.rs`

**Interfaces:** exercises `SemaNumber` and the two arithmetic paths against algebraic laws and each other.

- [ ] **Step 1:** Add `proptest`-style (or hand-rolled deterministic) tests in `crates/sema-core` asserting, over random tower values: commutativity/associativity of `add`/`mul`, `a - a == 0`, `a * (1/a) == 1` for nonzero exact `a`, `to_exact(to_inexact(exact_dyadic)) == exact_dyadic`, and that `add`/`mul`/`div` results are always normalized (a `Rational` never has denom 1, a `Complex` never has exact-zero imaginary). Use the existing test deps; if adding `proptest`, add it under `[dev-dependencies]`.
- [ ] **Step 2:** Add parity eval-tests that force the VM fast path and the stdlib path to agree on boundary values (`i64::MAX`, `i64::MIN`, just-over-i64, exact/inexact mixes). Since both paths pin the same literal oracle, a divergence fails one case.
- [ ] **Step 3:** Run `cargo test -p sema-core --test number_props && cargo test -p sema --test eval_test`. Commit.
```bash
git add crates/sema-core/tests/number_props.rs crates/sema/tests/eval_test.rs crates/sema-core/Cargo.toml Cargo.lock
git commit -m "test: numeric-tower property tests and VM/stdlib parity at boundaries"
```

### Task 7.2: Full CI-equivalent gate

**Files:** none (verification only)

- [ ] **Step 1:** Run the full release-equivalent suite (per `AGENTS.md`):
  `cargo test --workspace && jake examples && jake smoke-bytecode && jake lint && jake docs-check`
  Expected: all green. Investigate and fix any failure before proceeding.
- [ ] **Step 2:** Run the byte-level fuzzers briefly to catch reader/eval panics on the new literal surface: `jake fuzz.all` for a bounded duration. Expected: no crashes.
- [ ] **Step 3:** Update `CHANGELOG.md` with a "Full numeric tower (bignums, rationals, complex)" entry under a new version section. Commit.
```bash
git add CHANGELOG.md
git commit -m "changelog: full numeric tower (bignums, exact rationals, complex)"
```

**Phase 7 exit criteria:** `cargo test --workspace && jake examples && jake smoke-bytecode && jake lint && jake docs-check` all green; fuzzers clean; the tower is complete, correct, and documented.

---

## Self-Review

**Spec coverage** — the original ask ("full numeric tower + arbitrarily large numbers, 100% complete"):
- Arbitrary-precision integers → Phase 1 (representation, arithmetic, reader, VM, predicates, serialize).
- Exact rationals → Phase 2.
- Complex → Phase 3.
- Full literal grammar (radix, exactness) → Phase 4.
- Every numeric builtin generalized (comparison, rounding, abs/min/max, integer division family, expt, transcendentals, number↔string, exact-integer-sqrt, rationalize, bitwise) → Phase 5.
- Cross-cutting surfaces (JSON, fuzzer, docs, typed-array boundary) → Phase 6.
- Verification (properties, VM/stdlib parity, CI gate) → Phase 7.

**Known coupling to flag for the executor:**
- **Tasks 1.3 and 1.4 are co-dependent** for their test oracle (the `+`-overflow tests need the reader to parse bignum literals). Execute 1.4's reader change before running 1.3's eval-tests, or seed the expected values via a bignum-producing expression.
- **`from_number` grows across phases** — Phase 1 leaves `Rational`/`Complex` as `unreachable!`, Phase 2 fills `Rational`, Phase 3 fills `Complex`. A subagent executing Phase 2/3 must edit the existing `from_number`, not re-add it.
- **The trait-site list (Task 1.1 steps 3) recurs three times** (bignum, rational, complex). Each new `ValueView`/`ValueViewRef` variant makes the crate not compile until every exhaustive match has its arm — treat a compile error naming a match as the checklist of remaining sites, not a mistake.
- **Rounding literals in Tasks 5.7/6.1** (`rationalize`, JSON quoting) — the expected values are best-effort; the executor must run the implementation once, read the actual output, and pin that as the oracle (these are deterministic, just tedious to predict by hand).

**Placeholder scan:** every code step contains real code or an exact site+one-line-arm instruction; the two "sweep" phases (5) show the full pattern and per-family tests. No `TODO`/`handle edge cases`/`similar to above` without the accompanying code.

**Type consistency:** `SemaNumber`/`Complex` (number.rs), `Value::{from_bigint,as_bigint,rational,as_rational,complex,as_complex,as_number,from_number}`, and the `ValueView(Ref)::{BigInt,Rational,Complex}` variants are named identically everywhere they appear across phases.




