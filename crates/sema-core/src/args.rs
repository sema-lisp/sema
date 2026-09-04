//! Typed access to native-function arguments and options maps.
//!
//! A native function receives `&[Value]`; every argument it reads needs a type
//! check that names the function and the argument position when it fails.
//! [`ArgsExt`] is that check, once: `args.str_at(0, "string/split")?` replaces
//! the `as_str().ok_or_else(|| SemaError::type_error(...))?` pattern and gives
//! the same error [`SemaError::argument_type`] produces. [`OptionsExt`] does the
//! same for keyword-keyed option maps (`{:timeout 30 :model "x"}`), and
//! [`ResultExt`] attaches a "what failed" prefix to I/O and other errors.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::rc::Rc;

use crate::{SemaError, Value};

fn as_real(v: &Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_int().map(|n| n as f64))
}

fn missing(func: &str, index: usize, expected: &str) -> SemaError {
    SemaError::argument_type(func, index + 1, expected, &Value::nil())
}

/// Position-checked argument accessors for a native function's `&[Value]`.
/// `index` is 0-based; errors report the 1-based argument number.
pub trait ArgsExt {
    fn str_at(&self, index: usize, func: &str) -> Result<&str, SemaError>;
    fn int_at(&self, index: usize, func: &str) -> Result<i64, SemaError>;
    /// Any real number, widened to `f64`.
    fn float_at(&self, index: usize, func: &str) -> Result<f64, SemaError>;
    fn bool_at(&self, index: usize, func: &str) -> Result<bool, SemaError>;
    fn list_at(&self, index: usize, func: &str) -> Result<&[Value], SemaError>;
    /// A list or a vector.
    fn seq_at(&self, index: usize, func: &str) -> Result<&[Value], SemaError>;
    fn map_at(&self, index: usize, func: &str) -> Result<Rc<BTreeMap<Value, Value>>, SemaError>;
    fn keyword_at(&self, index: usize, func: &str) -> Result<String, SemaError>;
    fn symbol_at(&self, index: usize, func: &str) -> Result<String, SemaError>;
    fn bytes_at(&self, index: usize, func: &str) -> Result<&[u8], SemaError>;
    /// The argument, or `None` when it is absent or nil (an optional trailing argument).
    fn opt_at(&self, index: usize) -> Option<&Value>;
}

/// Look up `args[index]` and convert it with an accessor path (`Value::as_str`),
/// reporting the function name and 1-based position on failure. The accessor
/// is a path, not a closure, so borrowing accessors keep the slice lifetime.
macro_rules! at {
    ($self:ident, $index:ident, $func:ident, $expected:literal, $get:path) => {{
        let value = $self
            .get($index)
            .ok_or_else(|| missing($func, $index, $expected))?;
        $get(value).ok_or_else(|| SemaError::argument_type($func, $index + 1, $expected, value))
    }};
}

impl ArgsExt for [Value] {
    fn str_at(&self, index: usize, func: &str) -> Result<&str, SemaError> {
        at!(self, index, func, "string", Value::as_str)
    }
    fn int_at(&self, index: usize, func: &str) -> Result<i64, SemaError> {
        at!(self, index, func, "integer", Value::as_int)
    }
    fn float_at(&self, index: usize, func: &str) -> Result<f64, SemaError> {
        at!(self, index, func, "number", as_real)
    }
    fn bool_at(&self, index: usize, func: &str) -> Result<bool, SemaError> {
        at!(self, index, func, "boolean", Value::as_bool)
    }
    fn list_at(&self, index: usize, func: &str) -> Result<&[Value], SemaError> {
        at!(self, index, func, "list", Value::as_list)
    }
    fn seq_at(&self, index: usize, func: &str) -> Result<&[Value], SemaError> {
        at!(self, index, func, "list or vector", Value::as_seq)
    }
    fn map_at(&self, index: usize, func: &str) -> Result<Rc<BTreeMap<Value, Value>>, SemaError> {
        at!(self, index, func, "map", Value::as_map_rc)
    }
    fn keyword_at(&self, index: usize, func: &str) -> Result<String, SemaError> {
        at!(self, index, func, "keyword", Value::as_keyword)
    }
    fn symbol_at(&self, index: usize, func: &str) -> Result<String, SemaError> {
        at!(self, index, func, "symbol", Value::as_symbol)
    }
    fn bytes_at(&self, index: usize, func: &str) -> Result<&[u8], SemaError> {
        at!(self, index, func, "bytevector", Value::as_bytevector)
    }
    fn opt_at(&self, index: usize) -> Option<&Value> {
        self.get(index).filter(|v| !v.is_nil())
    }
}

/// Keyword-keyed option lookups: `{:model "x" :timeout 30}`. Implemented for
/// the map itself and for a `Value` that may hold one (a non-map `Value`
/// simply has no options).
pub trait OptionsExt {
    /// The value under `:key`, if present.
    fn opt(&self, key: &str) -> Option<Value>;

    fn opt_str(&self, key: &str) -> Option<String> {
        self.opt(key).and_then(|v| v.as_str().map(str::to_string))
    }
    fn opt_int(&self, key: &str) -> Option<i64> {
        self.opt(key).and_then(|v| v.as_int())
    }
    fn opt_f64(&self, key: &str) -> Option<f64> {
        self.opt(key).and_then(|v| as_real(&v))
    }
    fn opt_bool(&self, key: &str) -> Option<bool> {
        self.opt(key).and_then(|v| v.as_bool())
    }
    /// A keyword or string value, as its name.
    fn opt_name(&self, key: &str) -> Option<String> {
        self.opt(key)
            .and_then(|v| v.as_keyword().or_else(|| v.as_str().map(str::to_string)))
    }
    fn opt_seq(&self, key: &str) -> Option<Vec<Value>> {
        self.opt(key)
            .and_then(|v| v.as_seq().map(<[Value]>::to_vec))
    }
    fn opt_map(&self, key: &str) -> Option<Rc<BTreeMap<Value, Value>>> {
        self.opt(key).and_then(|v| v.as_map_rc())
    }
    /// `:key` present and truthy.
    fn flag(&self, key: &str) -> bool {
        self.opt(key).is_some_and(|v| v.is_truthy())
    }
}

impl OptionsExt for BTreeMap<Value, Value> {
    fn opt(&self, key: &str) -> Option<Value> {
        self.get(&Value::keyword(key)).cloned()
    }
}

impl OptionsExt for Value {
    fn opt(&self, key: &str) -> Option<Value> {
        self.as_map_ref()
            .and_then(|m| m.get(&Value::keyword(key)).cloned())
    }
}

/// Attach a "what failed" prefix when converting a foreign error into a
/// `SemaError`: `std::fs::read(p).io_ctx(format!("file/read {p}"))?`.
pub trait ResultExt<T> {
    /// An I/O failure: `SemaError::Io("{what}: {error}")`.
    fn io_ctx(self, what: impl Display) -> Result<T, SemaError>;
    /// Any other failure: `SemaError::eval("{what}: {error}")`.
    fn eval_ctx(self, what: impl Display) -> Result<T, SemaError>;
}

impl<T, E: Display> ResultExt<T> for Result<T, E> {
    fn io_ctx(self, what: impl Display) -> Result<T, SemaError> {
        self.map_err(|e| SemaError::Io(format!("{what}: {e}")))
    }
    fn eval_ctx(self, what: impl Display) -> Result<T, SemaError> {
        self.map_err(|e| SemaError::eval(format!("{what}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_argument_errors_name_function_and_position() {
        let args = [Value::string("a"), Value::int(2)];
        assert_eq!(args.str_at(0, "f").unwrap(), "a");
        assert_eq!(args.int_at(1, "f").unwrap(), 2);
        assert_eq!(args.float_at(1, "f").unwrap(), 2.0);
        let err = args.str_at(1, "my/fn").unwrap_err().to_string();
        assert!(err.contains("my/fn") && err.contains("string"), "{err}");
        let err = args.int_at(5, "my/fn").unwrap_err().to_string();
        assert!(err.contains("my/fn"), "{err}");
        assert!(args.opt_at(5).is_none());
        assert!([Value::nil()].opt_at(0).is_none());
    }

    #[test]
    fn options_read_keyword_keys_from_maps_and_values() {
        let mut m = BTreeMap::new();
        m.insert(Value::keyword("model"), Value::string("x"));
        m.insert(Value::keyword("n"), Value::int(3));
        m.insert(Value::keyword("t"), Value::float(0.5));
        m.insert(Value::keyword("on"), Value::bool(true));
        m.insert(Value::keyword("kind"), Value::keyword("fast"));
        assert_eq!(m.opt_str("model").as_deref(), Some("x"));
        assert_eq!(m.opt_int("n"), Some(3));
        assert_eq!(m.opt_f64("n"), Some(3.0));
        assert_eq!(m.opt_f64("t"), Some(0.5));
        assert!(m.flag("on"));
        assert!(!m.flag("missing"));
        assert_eq!(m.opt_name("kind").as_deref(), Some("fast"));
        let v = Value::map(m);
        assert_eq!(v.opt_str("model").as_deref(), Some("x"));
        assert_eq!(Value::int(1).opt_str("model"), None);
    }

    #[test]
    fn result_context_prefixes_the_message() {
        let r: Result<(), std::io::Error> =
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        let err = r.io_ctx("file/read x").unwrap_err().to_string();
        assert!(err.contains("file/read x: gone"), "{err}");
        let r: Result<(), String> = Err("bad".into());
        let err = r.eval_ctx("json/decode").unwrap_err().to_string();
        assert!(err.contains("json/decode: bad"), "{err}");
    }
}
