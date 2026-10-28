//! Conversions between Rust types and [`Value`] for typed host functions.
//!
//! [`FromValue`] extracts an argument; [`IntoValue`] builds a result. Both
//! back `register_fn_typed` and `register_async_fn_typed` in the public
//! `sema` crate, and can be used directly in hand-written host functions.

use crate::value::{Value, ValueViewRef};
use crate::SemaError;

/// Extract a Rust value from a Sema [`Value`].
pub trait FromValue: Sized {
    /// The Sema type name reported in a type error, e.g. `"int"`.
    fn expected() -> String;

    /// Convert, or return a type error naming [`expected`](Self::expected).
    fn from_value(value: &Value) -> Result<Self, SemaError>;
}

/// Convert a Rust value into a Sema [`Value`].
pub trait IntoValue {
    fn into_value(self) -> Value;
}

/// A keyword argument or result, by name without the `:` prefix.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Keyword(pub String);

/// A symbol argument or result, by name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(pub String);

fn mismatch<T: FromValue>(value: &Value) -> SemaError {
    // An int that is out of range, or a bignum, would otherwise read as
    // "expected int in 0..=255, got int"; include the value.
    let got = match value.view_ref() {
        ValueViewRef::Int(n) => format!("int {n}"),
        ValueViewRef::BigInt(n) => format!("int {n}"),
        _ => value.type_name().to_string(),
    };
    SemaError::type_error(T::expected(), got)
}

macro_rules! from_with {
    ($ty:ty, $expected:literal, |$v:ident| $body:expr) => {
        impl FromValue for $ty {
            fn expected() -> String {
                $expected.to_string()
            }
            fn from_value($v: &Value) -> Result<Self, SemaError> {
                ($body).ok_or_else(|| mismatch::<Self>($v))
            }
        }
    };
}

from_with!(i64, "int", |v| v.as_int());
from_with!(f64, "number", |v| v.as_float());
from_with!(bool, "bool", |v| v.as_bool());
from_with!(char, "char", |v| v.as_char());
from_with!(String, "string", |v| v.as_str().map(str::to_owned));
from_with!(Keyword, "keyword", |v| v.as_keyword().map(Keyword));
from_with!(Symbol, "symbol", |v| v.as_symbol().map(Symbol));

macro_rules! from_int {
    ($($ty:ty),*) => {$(
        impl FromValue for $ty {
            fn expected() -> String {
                format!("int in {}..={}", <$ty>::MIN, <$ty>::MAX)
            }
            fn from_value(value: &Value) -> Result<Self, SemaError> {
                value
                    .as_int()
                    .and_then(|n| <$ty>::try_from(n).ok())
                    .ok_or_else(|| mismatch::<Self>(value))
            }
        }
    )*};
}

from_int!(i32, u32, u64, usize, u8);

impl FromValue for Value {
    fn expected() -> String {
        "any".to_string()
    }
    fn from_value(value: &Value) -> Result<Self, SemaError> {
        Ok(value.clone())
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn expected() -> String {
        format!("{} or nil", T::expected())
    }
    fn from_value(value: &Value) -> Result<Self, SemaError> {
        if value.is_nil() {
            Ok(None)
        } else {
            T::from_value(value).map(Some)
        }
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    fn expected() -> String {
        format!("list or vector of {}", T::expected())
    }
    fn from_value(value: &Value) -> Result<Self, SemaError> {
        let items = match value.view_ref() {
            ValueViewRef::List(items) | ValueViewRef::Vector(items) => items,
            _ => return Err(mismatch::<Self>(value)),
        };
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                T::from_value(item).map_err(|error| error.with_note(format!("in element {index}")))
            })
            .collect()
    }
}

impl IntoValue for Value {
    fn into_value(self) -> Value {
        self
    }
}

impl IntoValue for () {
    fn into_value(self) -> Value {
        Value::nil()
    }
}

macro_rules! into_with {
    ($ty:ty, |$v:ident| $body:expr) => {
        impl IntoValue for $ty {
            fn into_value(self) -> Value {
                let $v = self;
                $body
            }
        }
    };
}

into_with!(i64, |v| Value::int(v));
into_with!(i32, |v| Value::int(v.into()));
into_with!(u32, |v| Value::int(v.into()));
into_with!(u8, |v| Value::int(v.into()));
into_with!(i16, |v| Value::int(v.into()));
into_with!(u16, |v| Value::int(v.into()));

macro_rules! into_wide {
    ($($ty:ty),*) => {$(
        impl IntoValue for $ty {
            fn into_value(self) -> Value {
                match i64::try_from(self) {
                    Ok(n) => Value::int(n),
                    Err(_) => Value::from_bigint(num_bigint::BigInt::from(self)),
                }
            }
        }
    )*};
}

into_wide!(u64, usize, isize, i128, u128);
into_with!(f64, |v| Value::float(v));
into_with!(bool, |v| Value::bool(v));
into_with!(char, |v| Value::char(v));
into_with!(String, |v| Value::string(&v));
into_with!(&str, |v| Value::string(v));
into_with!(Keyword, |v| Value::keyword(&v.0));
into_with!(Symbol, |v| Value::symbol(&v.0));

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        self.map_or_else(Value::nil, IntoValue::into_value)
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::list(self.into_iter().map(IntoValue::into_value).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(i64::from_value(&7_i64.into_value()).unwrap(), 7);
        assert_eq!(f64::from_value(&Value::int(2)).unwrap(), 2.0);
        assert_eq!(
            String::from_value(&"hi".into_value()).unwrap(),
            "hi".to_string()
        );
        assert_eq!(
            Keyword::from_value(&Keyword("status".into()).into_value()).unwrap(),
            Keyword("status".into())
        );
        assert_eq!(
            Option::<i64>::from_value(&Value::nil()).unwrap(),
            None::<i64>
        );
        assert_eq!(
            Vec::<i64>::from_value(&vec![1_i64, 2].into_value()).unwrap(),
            vec![1, 2]
        );
        assert_eq!(
            Vec::<i64>::from_value(&Value::vector(vec![Value::int(3)])).unwrap(),
            vec![3]
        );
    }

    #[test]
    fn type_errors_name_the_expected_type() {
        let error = i64::from_value(&Value::string("x")).unwrap_err();
        assert!(matches!(error.inner(), SemaError::Type { .. }), "{error}");
        assert!(u8::from_value(&Value::int(300)).is_err());
        let nested =
            Vec::<i64>::from_value(&Value::list(vec![Value::int(1), Value::nil()])).unwrap_err();
        assert!(format!("{nested:?}").contains("in element 1"), "{nested:?}");
    }
}
