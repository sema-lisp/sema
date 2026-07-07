//! Canonical conversions between `Value` and `serde_json::Value`.
//!
//! Two modes:
//! - **Strict** (`value_to_json`): errors on NaN/Infinity and unsupported types.
//! - **Lossy** (`value_to_json_lossy`): NaN/Infinity→null, unsupported→string.

use std::collections::BTreeMap;

use crate::number::SemaNumber;
use crate::{resolve, SemaError, Value, ValueView};

/// Convert a Sema Value to a JSON value, erroring on NaN/Infinity and unsupported types.
pub fn value_to_json(val: &Value) -> Result<serde_json::Value, SemaError> {
    // Grow the stack on demand: a deeply nested value would otherwise overflow
    // the OS thread stack here and abort the process.
    crate::stack::maybe_grow(|| match val.view() {
        ValueView::Nil => Ok(serde_json::Value::Null),
        ValueView::Bool(b) => Ok(serde_json::Value::Bool(b)),
        ValueView::Int(n) => Ok(serde_json::Value::Number(n.into())),
        ValueView::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| SemaError::eval("cannot encode NaN/Infinity as JSON")),
        // JSON's number syntax permits arbitrary-precision integers: emit the
        // bignum's decimal digits directly as a JSON number.
        ValueView::BigInt(n) => bignum_to_json_number(n.to_string()),
        // JSON has no rational or complex number form; emit the reader-
        // round-trippable string form (`"1/3"`, `"3+4i"`) so encoding never fails.
        ValueView::Rational(_) | ValueView::Complex(_) => {
            Ok(serde_json::Value::String(val.to_string()))
        }
        ValueView::String(s) => Ok(serde_json::Value::String(s.to_string())),
        ValueView::Keyword(s) => Ok(serde_json::Value::String(resolve(s))),
        ValueView::Symbol(s) => Ok(serde_json::Value::String(resolve(s))),
        ValueView::List(items) | ValueView::Vector(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(value_to_json).collect();
            Ok(serde_json::Value::Array(arr?))
        }
        ValueView::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(key_to_string(k), value_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        ValueView::HashMap(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(key_to_string(k), value_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        _ => Err(SemaError::eval(format!(
            "cannot encode {} as JSON",
            val.type_name()
        ))),
    })
}

/// Convert a Sema Value to JSON without erroring. NaN/Infinity become null,
/// unsupported types become their string representation.
pub fn value_to_json_lossy(val: &Value) -> serde_json::Value {
    crate::stack::maybe_grow(|| match val.view() {
        ValueView::Nil => serde_json::Value::Null,
        ValueView::Bool(b) => serde_json::Value::Bool(b),
        ValueView::Int(n) => serde_json::Value::Number(n.into()),
        ValueView::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        // JSON's number syntax permits arbitrary-precision integers: emit the
        // bignum's decimal digits directly as a JSON number.
        ValueView::BigInt(n) => {
            bignum_to_json_number(n.to_string()).unwrap_or(serde_json::Value::Null)
        }
        // JSON has no rational or complex number form; emit the reader-
        // round-trippable string form (`"1/3"`, `"3+4i"`).
        ValueView::Rational(_) | ValueView::Complex(_) => {
            serde_json::Value::String(val.to_string())
        }
        ValueView::String(s) => serde_json::Value::String(s.to_string()),
        ValueView::Keyword(s) => serde_json::Value::String(resolve(s)),
        ValueView::Symbol(s) => serde_json::Value::String(resolve(s)),
        ValueView::List(items) | ValueView::Vector(items) => {
            serde_json::Value::Array(items.iter().map(value_to_json_lossy).collect())
        }
        ValueView::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(key_to_string(k), value_to_json_lossy(v));
            }
            serde_json::Value::Object(obj)
        }
        ValueView::HashMap(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(key_to_string(k), value_to_json_lossy(v));
            }
            serde_json::Value::Object(obj)
        }
        _ => serde_json::Value::String(val.to_string()),
    })
}

/// Encode a bignum's decimal digit string as a raw JSON integer literal.
/// Requires serde_json's `arbitrary_precision` feature so the digits survive
/// the round-trip through `serde_json::Number` without collapsing to `f64`.
fn bignum_to_json_number(digits: String) -> Result<serde_json::Value, SemaError> {
    let n: serde_json::Number = serde_json::from_str(&digits)
        .map_err(|e| SemaError::eval(format!("cannot encode bignum as JSON: {e}")))?;
    Ok(serde_json::Value::Number(n))
}

/// Convert a JSON value to a Sema Value.
pub fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::nil(),
        serde_json::Value::Bool(b) => Value::bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::int(i)
            } else if !n.is_f64() {
                // An integer literal beyond i64 range: parse the exact decimal
                // digits (preserved verbatim by serde_json's arbitrary_precision
                // feature) into a bignum instead of losing precision to f64.
                match SemaNumber::parse_int_radix(n.as_str(), 10) {
                    Some(SemaNumber::Integer(big)) => Value::from_bigint(big),
                    _ => n.as_f64().map(Value::float).unwrap_or_else(Value::nil),
                }
            } else if let Some(f) = n.as_f64() {
                Value::float(f)
            } else {
                Value::nil()
            }
        }
        serde_json::Value::String(s) => Value::string(s),
        serde_json::Value::Array(arr) => Value::list(arr.iter().map(json_to_value).collect()),
        serde_json::Value::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(Value::keyword(k), json_to_value(v));
            }
            Value::map(map)
        }
    }
}

/// Extract a string key from a Value for use as a JSON/TOML map key.
pub fn key_to_string(k: &Value) -> String {
    match k.view() {
        ValueView::String(s) => s.to_string(),
        ValueView::Keyword(s) => resolve(s),
        ValueView::Symbol(s) => resolve(s),
        _ => k.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;

    #[test]
    fn test_lossy_preserves_map_structure_around_nan() {
        // A map with one normal value and one NaN — lossy should preserve
        // the map structure and only replace the NaN with null.
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("a"), Value::int(1));
        map.insert(Value::keyword("b"), Value::float(f64::NAN));
        let val = Value::map(map);

        let json = value_to_json_lossy(&val);

        // Must be an object, not a string
        assert!(json.is_object(), "expected JSON object, got: {json}");
        let obj = json.as_object().unwrap();
        assert_eq!(obj.get("a"), Some(&serde_json::Value::Number(1.into())));
        assert_eq!(obj.get("b"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn test_strict_errors_on_nan_in_map() {
        // Strict conversion should error when NaN is nested inside a map,
        // NOT stringify the whole map.
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("ok"), Value::int(42));
        map.insert(Value::keyword("bad"), Value::float(f64::NAN));
        let val = Value::map(map);

        let err = value_to_json(&val).unwrap_err();
        assert!(
            err.to_string().contains("NaN"),
            "expected NaN error, got: {err}"
        );
    }

    #[test]
    fn test_strict_errors_on_nan_in_list() {
        let val = Value::list(vec![Value::int(1), Value::float(f64::NAN)]);
        let err = value_to_json(&val).unwrap_err();
        assert!(err.to_string().contains("NaN"));
    }

    #[test]
    fn test_lossy_unsupported_type_becomes_string() {
        // A native function can't be represented in JSON; lossy should
        // stringify it rather than error.
        use crate::NativeFn;
        let val = Value::native_fn(NativeFn::simple("test-fn", |_| Ok(Value::nil())));
        let json = value_to_json_lossy(&val);
        assert!(json.is_string(), "expected string, got: {json}");
    }

    #[test]
    fn test_lossy_preserves_list_structure_around_nan() {
        let val = Value::list(vec![Value::int(1), Value::float(f64::NAN), Value::int(3)]);

        let json = value_to_json_lossy(&val);

        assert!(json.is_array(), "expected JSON array, got: {json}");
        let arr = json.as_array().unwrap();
        assert_eq!(arr[0], serde_json::Value::Number(1.into()));
        assert_eq!(arr[1], serde_json::Value::Null);
        assert_eq!(arr[2], serde_json::Value::Number(3.into()));
    }

    // ── value_to_json tests ──────────────────────────────────────────

    #[test]
    fn test_value_to_json_nil() {
        let json = value_to_json(&Value::nil()).unwrap();
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_value_to_json_bool() {
        assert_eq!(
            value_to_json(&Value::bool(true)).unwrap(),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            value_to_json(&Value::bool(false)).unwrap(),
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn test_value_to_json_int() {
        let json = value_to_json(&Value::int(42)).unwrap();
        assert_eq!(json, serde_json::Value::Number(42.into()));
    }

    #[test]
    fn test_value_to_json_float() {
        let json = value_to_json(&Value::float(3.14)).unwrap();
        assert_eq!(
            json,
            serde_json::Value::Number(serde_json::Number::from_f64(3.14).unwrap())
        );
    }

    #[test]
    fn test_value_to_json_string() {
        let json = value_to_json(&Value::string("hello")).unwrap();
        assert_eq!(json, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn test_value_to_json_keyword() {
        let json = value_to_json(&Value::keyword("foo")).unwrap();
        assert_eq!(json, serde_json::Value::String("foo".to_string()));
    }

    #[test]
    fn test_value_to_json_symbol() {
        let json = value_to_json(&Value::symbol("foo")).unwrap();
        assert_eq!(json, serde_json::Value::String("foo".to_string()));
    }

    #[test]
    fn test_value_to_json_list() {
        let val = Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]);
        let json = value_to_json(&val).unwrap();
        assert_eq!(
            json,
            serde_json::Value::Array(vec![
                serde_json::Value::Number(1.into()),
                serde_json::Value::Number(2.into()),
                serde_json::Value::Number(3.into()),
            ])
        );
    }

    #[test]
    fn test_value_to_json_map() {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("x"), Value::int(10));
        map.insert(Value::keyword("y"), Value::int(20));
        let val = Value::map(map);
        let json = value_to_json(&val).unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(obj.get("x"), Some(&serde_json::Value::Number(10.into())));
        assert_eq!(obj.get("y"), Some(&serde_json::Value::Number(20.into())));
    }

    // ── json_to_value tests ──────────────────────────────────────────

    #[test]
    fn test_json_to_value_null() {
        let val = json_to_value(&serde_json::Value::Null);
        assert!(val.is_nil());
    }

    #[test]
    fn test_json_to_value_int() {
        let val = json_to_value(&serde_json::json!(42));
        assert_eq!(val.as_int(), Some(42));
    }

    #[test]
    fn test_json_to_value_string() {
        let val = json_to_value(&serde_json::json!("hello"));
        assert_eq!(val.as_str(), Some("hello"));
    }

    #[test]
    fn test_json_to_value_array() {
        let val = json_to_value(&serde_json::json!([1, 2, 3]));
        let items = val.as_list().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_int(), Some(1));
        assert_eq!(items[1].as_int(), Some(2));
        assert_eq!(items[2].as_int(), Some(3));
    }

    #[test]
    fn test_json_to_value_object() {
        let val = json_to_value(&serde_json::json!({"name": "alice", "age": 30}));
        match val.view() {
            ValueView::Map(map) => {
                assert_eq!(
                    map.get(&Value::keyword("name")),
                    Some(&Value::string("alice"))
                );
                assert_eq!(map.get(&Value::keyword("age")), Some(&Value::int(30)));
            }
            _ => panic!("expected map"),
        }
    }

    // ── roundtrip test ───────────────────────────────────────────────

    #[test]
    fn test_json_roundtrip_simple() {
        let original = Value::list(vec![
            Value::int(1),
            Value::string("two"),
            Value::bool(true),
            Value::nil(),
        ]);
        let json = value_to_json(&original).unwrap();
        let roundtripped = json_to_value(&json);
        // List structure preserved
        let items = roundtripped.as_list().unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].as_int(), Some(1));
        assert_eq!(items[1].as_str(), Some("two"));
        assert_eq!(items[2].as_bool(), Some(true));
        assert!(items[3].is_nil());
    }

    // ── key_to_string tests ──────────────────────────────────────────

    #[test]
    fn test_key_to_string_keyword() {
        let s = key_to_string(&Value::keyword("foo"));
        assert_eq!(s, "foo");
    }

    #[test]
    fn test_key_to_string_string() {
        let s = key_to_string(&Value::string("bar"));
        assert_eq!(s, "bar");
    }

    #[test]
    fn test_key_to_string_int() {
        let s = key_to_string(&Value::int(42));
        assert_eq!(s, "42");
    }

    #[test]
    fn test_bigint_encodes_as_raw_json_digits() {
        // 2^127: well beyond i64/u64 range.
        let digits = "170141183460469231731687303715884105728";
        let big: num_bigint::BigInt = digits.parse().unwrap();
        let val = Value::from_bigint(big);

        let json = value_to_json(&val).unwrap();
        assert!(json.is_number(), "expected a JSON number, got: {json}");
        assert_eq!(serde_json::to_string(&json).unwrap(), digits);
    }

    #[test]
    fn test_bigint_decodes_exactly_from_raw_json_digits() {
        let digits = "170141183460469231731687303715884105728";
        let json: serde_json::Value = serde_json::from_str(digits).unwrap();

        let val = json_to_value(&json);

        let big: num_bigint::BigInt = digits.parse().unwrap();
        assert_eq!(val, Value::from_bigint(big));
    }

    #[test]
    fn test_rational_encodes_as_quoted_string() {
        let r = num_rational::BigRational::new(1.into(), 3.into());
        let val = Value::rational(r);

        let json = value_to_json(&val).unwrap();
        assert_eq!(json, serde_json::Value::String("1/3".to_string()));
    }
}
