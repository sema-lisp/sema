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
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

    /// Collapse to the tightest canonical form (see the type invariants).
    /// Cheap and idempotent; every lowering constructor and arithmetic result
    /// passes through it.
    pub fn normalize(self) -> SemaNumber {
        use num_traits::{One, Zero};
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

    /// Lossy projection to `f64` for inexact operations (`sqrt`, `sin`, mixed
    /// arithmetic). A `Complex` cannot project to a real — returns `f64::NAN`;
    /// callers that can receive complex must special-case it before calling.
    pub fn to_f64(&self) -> f64 {
        use num_traits::ToPrimitive;
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
        use num_traits::Zero;
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

    // `add`/`sub`/`mul`/`div`/`neg` deliberately mirror the `std::ops` method
    // names (this is the tower's public arithmetic interface, consumed by
    // later phases as `SemaNumber::add` etc.) rather than implementing the
    // traits, since `div` must return `Result` for divide-by-zero signalling.
    #[allow(clippy::should_implement_trait)]
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

    /// The exact square root of a non-negative exact perfect square, else
    /// `None`. Keeps `(sqrt 4) => 2` exact and lets `(sqrt -1) => +i` produce
    /// an exact imaginary part (R7RS). Inexact reals and non-squares yield
    /// `None`, leaving the caller to fall back to `f64::sqrt`.
    pub fn exact_sqrt(&self) -> Option<SemaNumber> {
        use num_traits::Signed;
        match self {
            SemaNumber::Integer(n) if !n.is_negative() => {
                let root = n.sqrt();
                (&root * &root == *n).then_some(SemaNumber::Integer(root))
            }
            SemaNumber::Rational(r) if !r.is_negative() => {
                // A reduced non-negative rational is a perfect square iff its
                // numerator and denominator are both perfect squares.
                let (rn, rd) = (r.numer().sqrt(), r.denom().sqrt());
                (&rn * &rn == *r.numer() && &rd * &rd == *r.denom())
                    .then(|| SemaNumber::Rational(BigRational::new(rn, rd)).normalize())
            }
            _ => None,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn add(self, other: SemaNumber) -> SemaNumber {
        let (a, b) = SemaNumber::promote(self, other);
        match (a, b) {
            (SemaNumber::Integer(x), SemaNumber::Integer(y)) => SemaNumber::Integer(x + y),
            (SemaNumber::Rational(x), SemaNumber::Rational(y)) => SemaNumber::Rational(x + y),
            (SemaNumber::Real(x), SemaNumber::Real(y)) => SemaNumber::Real(x + y),
            (SemaNumber::Complex(x), SemaNumber::Complex(y)) => {
                SemaNumber::Complex(Box::new(Complex {
                    re: x.re.add(y.re),
                    im: x.im.add(y.im),
                }))
            }
            _ => unreachable!("promote guarantees equal levels"),
        }
        .normalize()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, other: SemaNumber) -> SemaNumber {
        self.add(other.neg())
    }

    /// Non-negative magnitude. For a real number this is the usual absolute
    /// value (exactness-preserving: an exact input stays exact). For a
    /// `Complex`, this is its `f64` hypot — always inexact, since the tower
    /// has no exact square root in general.
    pub fn abs(self) -> SemaNumber {
        use num_traits::Signed;
        match self {
            SemaNumber::Integer(n) => SemaNumber::Integer(n.abs()),
            SemaNumber::Rational(r) => SemaNumber::Rational(r.abs()),
            SemaNumber::Real(f) => SemaNumber::Real(f.abs()),
            SemaNumber::Complex(c) => SemaNumber::Real(c.re.to_f64().hypot(c.im.to_f64())),
        }
        .normalize()
    }

    /// Round toward negative infinity. Exactness-preserving: an exact
    /// rational rounds to an exact `Integer`; an inexact `Real` stays a
    /// `Real`. Not meaningful for `Complex` — callers must guard with
    /// `is_real()` before calling (mirrors `cmp_real`'s contract).
    pub fn floor(self) -> SemaNumber {
        match self {
            SemaNumber::Integer(n) => SemaNumber::Integer(n),
            SemaNumber::Rational(r) => SemaNumber::Integer(r.floor().to_integer()),
            SemaNumber::Real(f) => SemaNumber::Real(f.floor()),
            SemaNumber::Complex(_) => unreachable!("caller must guard complex via is_real()"),
        }
    }

    /// Round toward positive infinity. See `floor` for exactness rules.
    pub fn ceil(self) -> SemaNumber {
        match self {
            SemaNumber::Integer(n) => SemaNumber::Integer(n),
            SemaNumber::Rational(r) => SemaNumber::Integer(r.ceil().to_integer()),
            SemaNumber::Real(f) => SemaNumber::Real(f.ceil()),
            SemaNumber::Complex(_) => unreachable!("caller must guard complex via is_real()"),
        }
    }

    /// Round toward zero (truncate the fractional part). See `floor` for
    /// exactness rules.
    pub fn truncate(self) -> SemaNumber {
        match self {
            SemaNumber::Integer(n) => SemaNumber::Integer(n),
            SemaNumber::Rational(r) => SemaNumber::Integer(r.trunc().to_integer()),
            SemaNumber::Real(f) => SemaNumber::Real(f.trunc()),
            SemaNumber::Complex(_) => unreachable!("caller must guard complex via is_real()"),
        }
    }

    /// Round to the nearest integer, ties to even (R7RS "banker's rounding").
    /// See `floor` for exactness rules.
    pub fn round(self) -> SemaNumber {
        use num_integer::Integer;
        use std::cmp::Ordering;
        match self {
            SemaNumber::Integer(n) => SemaNumber::Integer(n),
            SemaNumber::Rational(r) => {
                let (numer, denom) = (r.numer().clone(), r.denom().clone());
                // denom is always positive (BigRational's invariant), so
                // div_floor/rem here behave like ordinary floored division.
                let floor = numer.div_floor(&denom);
                let rem = &numer - &floor * &denom; // in [0, denom)
                let twice_rem = &rem * BigInt::from(2);
                let round_up = match twice_rem.cmp(&denom) {
                    Ordering::Less => false,
                    Ordering::Greater => true,
                    // Exact tie: round to whichever neighbor is even.
                    Ordering::Equal => floor.is_odd(),
                };
                let result = if round_up { floor + 1 } else { floor };
                SemaNumber::Integer(result)
            }
            SemaNumber::Real(f) => SemaNumber::Real(f.round_ties_even()),
            SemaNumber::Complex(_) => unreachable!("caller must guard complex via is_real()"),
        }
    }

    #[allow(clippy::should_implement_trait)]
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

/// Returned by `SemaNumber::div` when dividing by an *exact* zero. An inexact
/// zero divisor follows IEEE-754 (→ ±inf / NaN), matching Scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DivByZero;

impl SemaNumber {
    #[allow(clippy::should_implement_trait)]
    pub fn div(self, other: SemaNumber) -> Result<SemaNumber, DivByZero> {
        use num_traits::Zero;
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
                let denom =
                    y.re.clone()
                        .mul(y.re.clone())
                        .add(y.im.clone().mul(y.im.clone()));
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

    /// Raise `self` to an arbitrary-precision integer exponent via repeated
    /// squaring — O(log |exp|) multiplications, so `(expt 2 100)` costs ~7
    /// squarings rather than 100. A negative exponent yields the reciprocal
    /// (an exact base stays exact: `2^-3 => 1/8`); base 0 with a negative
    /// exponent divides by zero. Only meaningful for exact/inexact reals —
    /// callers pick this path themselves (float exponents fall back to
    /// `f64::powf` at the builtin layer).
    pub fn powi(self, exp: &BigInt) -> Result<SemaNumber, DivByZero> {
        use num_integer::Integer;
        use num_traits::{Signed, Zero};
        let negative = exp.is_negative();
        let mut e = if negative { -exp } else { exp.clone() };
        let mut result = SemaNumber::from_i64(1);
        let mut base = self;
        while !e.is_zero() {
            if e.is_odd() {
                result = result.mul(base.clone());
            }
            e = e.div_floor(&BigInt::from(2));
            if !e.is_zero() {
                base = base.clone().mul(base);
            }
        }
        if negative {
            SemaNumber::from_i64(1).div(result)
        } else {
            Ok(result)
        }
    }
}

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
            _ => self.cmp_real(other) == Some(std::cmp::Ordering::Equal),
        }
    }

    /// Ordering for real numbers. `None` if either operand is complex or a NaN.
    /// Exact-vs-inexact converts the float to an exact rational so the compare
    /// is precise even above 2^53.
    pub fn cmp_real(&self, other: &SemaNumber) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        if matches!(self, SemaNumber::Complex(_)) || matches!(other, SemaNumber::Complex(_)) {
            return None;
        }
        // If both inexact, compare as f64 (preserves NaN → None).
        if let (SemaNumber::Real(x), SemaNumber::Real(y)) = (self, other) {
            return x.partial_cmp(y);
        }
        // Fast path for the infinity/NaN cases: if exactly one side is a
        // non-finite Real, its sign decides.
        match (self, other) {
            (SemaNumber::Real(f), _) if !f.is_finite() => {
                return if f.is_nan() {
                    None
                } else if *f > 0.0 {
                    Some(Ordering::Greater)
                } else {
                    Some(Ordering::Less)
                };
            }
            (_, SemaNumber::Real(f)) if !f.is_finite() => {
                return if f.is_nan() {
                    None
                } else if *f > 0.0 {
                    Some(Ordering::Less)
                } else {
                    Some(Ordering::Greater)
                };
            }
            _ => {}
        }
        // Mixed or both-exact: lift any (finite) Real to an exact rational.
        let to_exact = |v: &SemaNumber| -> SemaNumber {
            match v {
                SemaNumber::Real(f) => {
                    SemaNumber::real_to_exact(*f).expect("finite (checked above)")
                }
                other => other.clone(),
            }
        };
        let a = to_exact(self);
        let b = to_exact(other);
        let (a, b) = SemaNumber::promote(a, b);
        match (a, b) {
            (SemaNumber::Integer(x), SemaNumber::Integer(y)) => Some(x.cmp(&y)),
            (SemaNumber::Rational(x), SemaNumber::Rational(y)) => Some(x.cmp(&y)),
            _ => unreachable!("both exact after real_to_exact + promote"),
        }
    }
}

/// Format a real component the way Sema prints floats/ints (shared by the
/// complex arm so `2.0+0.5i` matches standalone `2.0`/`0.5`).
fn fmt_real(n: &SemaNumber, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

impl std::fmt::Display for SemaNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use num_traits::Zero;
        match self {
            SemaNumber::Complex(c) => {
                fmt_real(&c.re, f)?;
                // Explicit sign then magnitude, so `0-1i` reads back correctly.
                let (sign, mag) = match &c.im {
                    SemaNumber::Integer(v) if *v < BigInt::zero() => {
                        ('-', SemaNumber::Integer(-v.clone()))
                    }
                    SemaNumber::Rational(r) if *r < BigRational::from(BigInt::zero()) => {
                        ('-', SemaNumber::Rational(-r.clone()))
                    }
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
            SemaNumber::Real(f) => SemaNumber::real_to_exact(f)
                .map(|n| n.normalize())
                .unwrap_or(SemaNumber::Real(f)),
            SemaNumber::Complex(c) => SemaNumber::Complex(Box::new(Complex {
                re: c.re.to_exact(),
                im: c.im.to_exact(),
            }))
            .normalize(),
            exact => exact,
        }
    }
}

/// The simplest rational number in the closed interval `[lo, hi]` (requires
/// `lo <= hi`): the one with the smallest denominator, and among those the
/// smallest numerator magnitude. Descends the Stern–Brocot tree via the
/// continued-fraction expansion of the endpoints — this is the mathematical
/// core of R7RS `rationalize`.
fn simplest_rational_in(lo: BigRational, hi: BigRational) -> BigRational {
    use num_traits::{Signed, Zero};
    if lo.is_positive() {
        simplest_positive(lo, hi)
    } else if hi.is_negative() {
        // Reflect the interval into the positive reals, solve, negate back.
        -simplest_positive(-hi, -lo)
    } else {
        // 0 lies in `[lo, hi]` and is the simplest rational of all.
        BigRational::zero()
    }
}

/// Simplest rational in `[lo, hi]` assuming `0 < lo <= hi`.
fn simplest_positive(lo: BigRational, hi: BigRational) -> BigRational {
    use num_traits::One;
    let fl = lo.floor(); // integer-valued BigRational
    if fl == lo {
        // `lo` is itself an integer — the simplest value in the interval.
        fl
    } else if fl < hi.floor() {
        // An integer strictly above `lo` still lies at/below `hi`.
        fl + BigRational::one()
    } else {
        // `lo` and `hi` share an integer part; recurse on the reciprocals of
        // their fractional parts (reciprocation flips the ordering, so the new
        // lower bound comes from `hi`).
        let frac_lo = &lo - &fl;
        let frac_hi = &hi - &fl;
        fl + simplest_positive(frac_hi.recip(), frac_lo.recip()).recip()
    }
}

impl SemaNumber {
    /// Exact `BigRational` value of a *real* tower number, or `None` for a
    /// non-finite `Real` (±inf / NaN) or a `Complex`.
    fn to_exact_rational(&self) -> Option<BigRational> {
        match self {
            SemaNumber::Integer(i) => Some(BigRational::from(i.clone())),
            SemaNumber::Rational(r) => Some(r.clone()),
            SemaNumber::Real(f) => BigRational::from_float(*f),
            SemaNumber::Complex(_) => None,
        }
    }

    /// The simplest rational number within `tol` of `self` (R7RS
    /// `rationalize`). Both operands must be real (the builtin guards complex).
    /// Computes exactly over `BigRational` — the interval is
    /// `[self - |tol|, self + |tol|]` — then applies inexactness contagion:
    /// the result is inexact iff either operand is inexact.
    pub fn rationalize(&self, tol: &SemaNumber) -> SemaNumber {
        use num_traits::{Signed, Zero};
        let inexact = !self.is_exact() || !tol.is_exact();
        let wrap = |r: BigRational| {
            let n = SemaNumber::Rational(r).normalize();
            if inexact {
                n.to_inexact()
            } else {
                n
            }
        };
        // A non-finite value has no meaningful nearby rational — return it as-is.
        let xr = match self.to_exact_rational() {
            Some(x) => x,
            None => return self.clone(),
        };
        let er = match tol.to_exact_rational() {
            Some(e) => e.abs(),
            // An infinite tolerance admits every rational ⇒ 0 is simplest; a
            // NaN tolerance has no interval ⇒ leave the value unchanged.
            None => {
                return if matches!(tol, SemaNumber::Real(f) if f.is_infinite()) {
                    wrap(BigRational::zero())
                } else {
                    self.clone()
                };
            }
        };
        let lo = &xr - &er;
        let hi = &xr + &er;
        wrap(simplest_rational_in(lo, hi))
    }
}

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
            Integer(n) => {
                0u8.hash(state);
                n.hash(state);
            }
            Rational(r) => {
                1u8.hash(state);
                r.hash(state);
            }
            Real(f) => {
                2u8.hash(state);
                f.to_bits().hash(state);
            }
            Complex(c) => {
                3u8.hash(state);
                c.re.hash(state);
                c.im.hash(state);
            }
        }
    }
}
impl Ord for SemaNumber {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use SemaNumber::*;
        self.level()
            .cmp(&other.level())
            .then_with(|| match (self, other) {
                (Integer(a), Integer(b)) => a.cmp(b),
                (Rational(a), Rational(b)) => a.cmp(b),
                (Real(a), Real(b)) => a.total_cmp(b),
                (Complex(a), Complex(b)) => a.re.cmp(&b.re).then_with(|| a.im.cmp(&b.im)),
                _ => std::cmp::Ordering::Equal, // different levels already decided by level().cmp
            })
    }
}
impl PartialOrd for SemaNumber {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl SemaNumber {
    /// Parse an integer of arbitrary size in the given radix (2..=36). Accepts
    /// an optional leading `+`/`-`. Returns `None` on any invalid digit.
    pub fn parse_int_radix(digits: &str, radix: u32) -> Option<SemaNumber> {
        let (sign, body) = match digits.strip_prefix('-') {
            Some(rest) => (num_bigint::Sign::Minus, rest),
            None => (
                num_bigint::Sign::Plus,
                digits.strip_prefix('+').unwrap_or(digits),
            ),
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
        use num_traits::Zero;
        use std::str::FromStr;
        let (n, d) = s.split_once('/')?;
        let numer = BigInt::from_str(n).ok()?;
        let denom = BigInt::from_str(d).ok()?;
        if denom.is_zero() {
            return None;
        }
        Some(SemaNumber::Rational(BigRational::new(numer, denom)).normalize())
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
        let (a, b) =
            SemaNumber::promote(SemaNumber::Integer(BigInt::from(2)), SemaNumber::Real(0.5));
        assert!(matches!(a, SemaNumber::Real(_)));
        assert!(matches!(b, SemaNumber::Real(_)));
    }

    #[test]
    fn add_sub_mul_neg() {
        use num_traits::Zero;
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
        assert!(matches!(
            two().add(SemaNumber::Real(0.5)),
            SemaNumber::Real(_)
        ));
    }

    #[test]
    fn abs_preserves_exactness_over_reals_inexact_over_complex() {
        let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
        // exact integer stays exact
        assert!(matches!(n(-5).abs(), SemaNumber::Integer(v) if v == BigInt::from(5)));
        assert!(matches!(n(5).abs(), SemaNumber::Integer(v) if v == BigInt::from(5)));
        // exact rational stays exact
        let neg_half = SemaNumber::Rational(BigRational::new(BigInt::from(-1), BigInt::from(2)));
        assert!(matches!(neg_half.abs(),
            SemaNumber::Rational(r) if r == BigRational::new(BigInt::one(), BigInt::from(2))));
        // inexact real stays inexact
        assert!(matches!(SemaNumber::Real(-2.5).abs(), SemaNumber::Real(f) if f == 2.5));
        // complex magnitude is the (inexact) hypot of its components
        let c = SemaNumber::Complex(Box::new(Complex { re: n(3), im: n(4) }));
        assert!(matches!(c.abs(), SemaNumber::Real(f) if f == 5.0));
    }

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
        assert!(matches!(
            n(1).div(SemaNumber::Real(2.0)).unwrap(),
            SemaNumber::Real(_)
        ));
        // divide by exact zero → error
        assert!(n(1).div(n(0)).is_err());
        // divide by inexact zero → real infinity (IEEE), NOT an error
        assert!(
            matches!(n(1).div(SemaNumber::Real(0.0)).unwrap(), SemaNumber::Real(f) if f.is_infinite())
        );
    }

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
        assert_eq!(
            big.cmp_real(&SemaNumber::Real(9_007_199_254_740_992.0)),
            Some(Ordering::Greater)
        );
        // complex is unordered
        let i = SemaNumber::Complex(Box::new(Complex { re: n(0), im: n(1) }));
        assert_eq!(i.cmp_real(&n(0)), None);
        assert!(!i.num_eq(&n(0)));
    }

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
        let c2 = SemaNumber::Complex(Box::new(Complex {
            re: n(0),
            im: n(-1),
        }));
        assert_eq!(c2.to_string(), "0-1i");
    }

    #[test]
    fn exactness_conversions() {
        let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
        // exact → inexact
        assert!(matches!(n(3).to_inexact(), SemaNumber::Real(f) if f == 3.0));
        // inexact 0.5 → exact 1/2
        assert!(matches!(SemaNumber::Real(0.5).to_exact(),
            SemaNumber::Rational(r) if r == BigRational::new(BigInt::one(), BigInt::from(2))));
        // inexact 2.0 → exact 2 (normalizes to Integer)
        assert!(
            matches!(SemaNumber::Real(2.0).to_exact(), SemaNumber::Integer(k) if k == BigInt::from(2))
        );
        // bridges
        assert!(matches!(SemaNumber::from_i64(5), SemaNumber::Integer(k) if k == BigInt::from(5)));
        assert!(matches!(SemaNumber::from_f64(1.5), SemaNumber::Real(f) if f == 1.5));
    }

    #[test]
    fn parse_literals() {
        // arbitrary-precision decimal beyond i64
        let big =
            SemaNumber::parse_int_radix("170141183460469231731687303715884105728", 10).unwrap();
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
        assert!(
            matches!(SemaNumber::parse_rational("6/3").unwrap(), SemaNumber::Integer(n) if n == BigInt::from(2))
        );
        // rejects garbage
        assert!(SemaNumber::parse_rational("1/0").is_none()); // zero denominator
        assert!(SemaNumber::parse_int_radix("xyz", 16).is_none());
    }

    #[test]
    fn rounding_preserves_exactness() {
        let r = |n: i64, d: i64| {
            SemaNumber::Rational(BigRational::new(BigInt::from(n), BigInt::from(d)))
        };
        // 7/2 = 3.5
        assert!(matches!(r(7, 2).floor(), SemaNumber::Integer(n) if n == BigInt::from(3)));
        assert!(matches!(r(7, 2).ceil(), SemaNumber::Integer(n) if n == BigInt::from(4)));
        // banker's rounding: ties round to the nearest EVEN integer.
        assert!(matches!(r(7, 2).round(), SemaNumber::Integer(n) if n == BigInt::from(4))); // 3.5 -> 4
        assert!(matches!(r(5, 2).round(), SemaNumber::Integer(n) if n == BigInt::from(2))); // 2.5 -> 2
        assert!(matches!(r(-5, 2).round(), SemaNumber::Integer(n) if n == BigInt::from(-2))); // -2.5 -> -2
        assert!(matches!(r(-7, 2).truncate(), SemaNumber::Integer(n) if n == BigInt::from(-3)));
        // integers pass through unchanged.
        assert!(
            matches!(SemaNumber::Integer(BigInt::from(5)).floor(), SemaNumber::Integer(n) if n == BigInt::from(5))
        );
        // reals stay inexact.
        assert!(matches!(SemaNumber::Real(2.5).floor(), SemaNumber::Real(f) if f == 2.0));
        assert!(matches!(SemaNumber::Real(2.5).round(), SemaNumber::Real(f) if f == 2.0));
    }

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

    #[test]
    fn rationalize_simplest_in_interval() {
        let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
        let r = |a: i64, b: i64| {
            SemaNumber::Rational(BigRational::new(BigInt::from(a), BigInt::from(b)))
        };
        // 1/3 within 1/100 is already the simplest rational at that scale.
        assert_eq!(r(1, 3).rationalize(&r(1, 100)), r(1, 3));
        // Classic Scheme example: rationalize(3/10, 1/10) => 1/3.
        assert_eq!(r(3, 10).rationalize(&r(1, 10)), r(1, 3));
        // Exact inputs stay exact.
        assert!(r(1, 3).rationalize(&r(1, 100)).is_exact());
        // rationalize(5, 4): interval [1, 9] — simplest is the integer 1.
        assert_eq!(n(5).rationalize(&n(4)), n(1));
        // A tolerance spanning zero admits 0, the simplest rational of all.
        assert_eq!(n(3).rationalize(&n(5)), n(0));
        // Inexactness contagion: an inexact operand yields an inexact result.
        assert!(!SemaNumber::Real(0.3).rationalize(&r(1, 10)).is_exact());
        // A negative value: rationalize(-3/10, 1/10) => -1/3 (mirror image).
        assert_eq!(r(-3, 10).rationalize(&r(1, 10)), r(-1, 3));
    }

    #[test]
    fn powi_repeated_squaring() {
        let n = |v: i64| SemaNumber::Integer(BigInt::from(v));
        // 2^100 is exact and beyond i64 range.
        let expected: BigInt = "1267650600228229401496703205376".parse().unwrap();
        assert!(
            matches!(n(2).powi(&BigInt::from(100)).unwrap(), SemaNumber::Integer(v) if v == expected)
        );
        // negative exponent -> exact reciprocal rational
        assert!(matches!(n(2).powi(&BigInt::from(-3)).unwrap(),
            SemaNumber::Rational(r) if r == BigRational::new(BigInt::one(), BigInt::from(8))));
        // rational base
        let half = SemaNumber::Rational(BigRational::new(BigInt::one(), BigInt::from(2)));
        assert!(matches!(half.powi(&BigInt::from(3)).unwrap(),
            SemaNumber::Rational(r) if r == BigRational::new(BigInt::one(), BigInt::from(8))));
        // exponent 0 -> exact 1, even for base 0
        assert!(
            matches!(n(0).powi(&BigInt::from(0)).unwrap(), SemaNumber::Integer(v) if v == BigInt::one())
        );
        // 0 base with negative exponent divides by zero
        assert!(n(0).powi(&BigInt::from(-1)).is_err());
    }
}
