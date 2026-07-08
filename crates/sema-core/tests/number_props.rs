//! Property-style tests for the `SemaNumber` tower.
//!
//! These are deterministic (no randomness, no `proptest`): they enumerate a
//! fixed spread of tower values — small/large exact integers, reduced and
//! unreduced rationals, inexact reals, and complex numbers — and assert the
//! algebraic laws every arithmetic op must obey. Because the tower is the
//! single arithmetic currency behind both the VM fast paths and the stdlib
//! builtins, a law broken here is a bug in every consumer.

use num_bigint::BigInt;
use num_traits::One;
use sema_core::number::{Complex, SemaNumber};

fn int(v: i64) -> SemaNumber {
    SemaNumber::from_i64(v)
}

fn big(digits: &str) -> SemaNumber {
    SemaNumber::parse_int_radix(digits, 10).unwrap()
}

fn rat(s: &str) -> SemaNumber {
    SemaNumber::parse_rational(s).unwrap()
}

fn real(f: f64) -> SemaNumber {
    SemaNumber::from_f64(f)
}

fn cplx(re: SemaNumber, im: SemaNumber) -> SemaNumber {
    SemaNumber::Complex(Box::new(Complex { re, im }))
}

/// A spread of exact values, safe to divide/invert (all nonzero except the
/// explicit zeros used where a law tolerates them).
fn exact_values() -> Vec<SemaNumber> {
    vec![
        int(0),
        int(1),
        int(-1),
        int(2),
        int(-7),
        int(1000),
        big("170141183460469231731687303715884105728"), // 2^127, well past i64
        big("-99999999999999999999999999999"),
        rat("1/3"),
        rat("-5/2"),
        rat("22/7"),
        rat("6/4"), // reduces to 3/2
        cplx(int(3), int(4)),
        cplx(int(0), int(1)),
        cplx(rat("1/2"), int(-2)),
    ]
}

/// Exact values plus inexact reals and a complex-with-real-components, to
/// exercise exactness contagion in the commutativity/associativity laws.
fn all_values() -> Vec<SemaNumber> {
    let mut v = exact_values();
    v.push(real(0.5));
    v.push(real(-3.25));
    v.push(real(2.0));
    v.push(cplx(real(1.5), real(2.5)));
    v
}

/// Nonzero exact values (for `a * (1/a) == 1` and division laws).
fn nonzero_exact() -> Vec<SemaNumber> {
    exact_values()
        .into_iter()
        .filter(|n| !n.num_eq(&int(0)))
        .collect()
}

/// Structural check of the tower's normalization invariants:
/// - a `Rational` never has denominator 1,
/// - a `Complex` never has an exact-zero imaginary part,
/// - `Complex` components are themselves never `Complex` and are normalized.
fn is_normalized(n: &SemaNumber) -> bool {
    match n {
        SemaNumber::Integer(_) | SemaNumber::Real(_) => true,
        SemaNumber::Rational(r) => !r.denom().is_one(),
        SemaNumber::Complex(c) => {
            let im_is_exact_zero = matches!(&c.im, SemaNumber::Integer(z) if *z == BigInt::from(0));
            let components_are_real = c.re.is_real() && c.im.is_real();
            !im_is_exact_zero && components_are_real && is_normalized(&c.re) && is_normalized(&c.im)
        }
    }
}

#[test]
fn add_is_commutative() {
    for a in all_values() {
        for b in all_values() {
            let ab = a.clone().add(b.clone());
            let ba = b.clone().add(a.clone());
            assert!(
                ab.num_eq(&ba),
                "add not commutative: {a} + {b} = {ab} but {b} + {a} = {ba}"
            );
        }
    }
}

#[test]
fn mul_is_commutative() {
    for a in all_values() {
        for b in all_values() {
            let ab = a.clone().mul(b.clone());
            let ba = b.clone().mul(a.clone());
            assert!(
                ab.num_eq(&ba),
                "mul not commutative: {a} * {b} = {ab} but {b} * {a} = {ba}"
            );
        }
    }
}

#[test]
fn add_is_associative_over_exact() {
    // Restricted to exact values so floating-point rounding does not break the
    // law (association order changes rounding for inexact reals).
    for a in exact_values() {
        for b in exact_values() {
            for c in exact_values() {
                let left = a.clone().add(b.clone()).add(c.clone());
                let right = a.clone().add(b.clone().add(c.clone()));
                assert!(
                    left.num_eq(&right),
                    "add not associative: ({a}+{b})+{c} = {left} vs {a}+({b}+{c}) = {right}"
                );
            }
        }
    }
}

#[test]
fn mul_is_associative_over_exact() {
    for a in exact_values() {
        for b in exact_values() {
            for c in exact_values() {
                let left = a.clone().mul(b.clone()).mul(c.clone());
                let right = a.clone().mul(b.clone().mul(c.clone()));
                assert!(
                    left.num_eq(&right),
                    "mul not associative: ({a}*{b})*{c} = {left} vs {a}*({b}*{c}) = {right}"
                );
            }
        }
    }
}

/// A number that is zero-valued. A complex whose components are both inexact
/// zeros (`0.0+0.0i`, e.g. from subtracting an inexact complex from itself)
/// stays complex per R7RS, so it is not `num_eq` to the exact integer 0 — but
/// it is still zero-valued component-wise.
fn is_numeric_zero(n: &SemaNumber) -> bool {
    match n {
        SemaNumber::Complex(c) => c.re.num_eq(&int(0)) && c.im.num_eq(&int(0)),
        other => other.num_eq(&int(0)),
    }
}

#[test]
fn a_minus_a_is_zero() {
    for a in all_values() {
        let d = a.clone().sub(a.clone());
        assert!(is_numeric_zero(&d), "a - a != 0 for a = {a}: got {d}");
    }
}

#[test]
fn a_times_reciprocal_is_one() {
    for a in nonzero_exact() {
        let recip = int(1).div(a.clone()).expect("nonzero divisor");
        let prod = a.clone().mul(recip.clone());
        assert!(
            prod.num_eq(&int(1)),
            "a * (1/a) != 1 for a = {a}: 1/a = {recip}, product = {prod}"
        );
    }
}

#[test]
fn to_exact_round_trips_dyadic() {
    // Exact dyadic rationals (denominator a power of two) are represented
    // exactly by f64, so exact -> inexact -> exact must recover the original.
    let dyadic = [
        int(0),
        int(1),
        int(-42),
        rat("1/2"),
        rat("-3/4"),
        rat("5/8"),
        rat("13/16"),
        rat("-7/32"),
    ];
    for x in dyadic {
        let round = x.clone().to_inexact().to_exact();
        assert!(
            round.num_eq(&x),
            "to_exact(to_inexact({x})) = {round}, expected {x}"
        );
        assert!(round.is_exact(), "round-trip result not exact for {x}");
    }
}

#[test]
fn arithmetic_results_are_normalized() {
    for a in all_values() {
        for b in all_values() {
            let sum = a.clone().add(b.clone());
            assert!(
                is_normalized(&sum),
                "add result not normalized: {a} + {b} = {sum}"
            );

            let prod = a.clone().mul(b.clone());
            assert!(
                is_normalized(&prod),
                "mul result not normalized: {a} * {b} = {prod}"
            );

            if let Ok(q) = a.clone().div(b.clone()) {
                assert!(
                    is_normalized(&q),
                    "div result not normalized: {a} / {b} = {q}"
                );
            }
        }
    }
}

#[test]
fn negation_round_trips() {
    for a in all_values() {
        let back = a.clone().neg().neg();
        assert!(back.num_eq(&a), "-(-a) != a for a = {a}: got {back}");
    }
}
