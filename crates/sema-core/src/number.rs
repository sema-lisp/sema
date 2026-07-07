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
        let (a, b) = SemaNumber::promote(SemaNumber::Integer(BigInt::from(2)), SemaNumber::Real(0.5));
        assert!(matches!(a, SemaNumber::Real(_)));
        assert!(matches!(b, SemaNumber::Real(_)));
    }
}
