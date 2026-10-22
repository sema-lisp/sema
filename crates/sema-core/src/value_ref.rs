//! A public, matchable view of a [`Value`] for embedders.
//!
//! [`Value`] is NaN-boxed and cannot be matched on directly. [`ValueRef`]
//! borrows a value and exposes the variants an embedder receives from Sema
//! code, so host functions can match their arguments with slice patterns:
//!
//! ```
//! use sema_core::{SemaError, Value, ValueRef};
//!
//! fn describe(args: &[Value]) -> Result<String, SemaError> {
//!     match ValueRef::slice(args).as_slice() {
//!         [] => Ok("no arguments".into()),
//!         [ValueRef::Keyword(k), ValueRef::Int(n)] if *k == "retry" => Ok(format!("retry {n}")),
//!         [ValueRef::Keyword(k), ValueRef::Int(n)] => Ok(format!("{k} = {n}")),
//!         [ValueRef::String(s)] => Ok(format!("text {s:?}")),
//!         other => Err(SemaError::eval(format!("unexpected {} arguments", other.len()))),
//!     }
//! }
//!
//! let args = [Value::keyword("retry"), Value::int(3)];
//! assert_eq!(describe(&args).unwrap(), "retry 3");
//! ```
//!
//! The enum is `#[non_exhaustive]`: values without a dedicated variant
//! (procedures, promises, records, big numbers, …) arrive as
//! [`ValueRef::Other`], which still carries the original [`Value`].

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use lasso::Spur;

use crate::value::{with_resolved, Value, ValueViewRef};

/// A borrowed, matchable view of a [`Value`]. Build one with
/// [`ValueRef::of`] or a whole argument list with [`ValueRef::slice`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub enum ValueRef<'a> {
    Nil,
    Bool(bool),
    /// A fixnum that fits in `i64`. Bignums arrive as [`ValueRef::Other`].
    Int(i64),
    Float(f64),
    Char(char),
    String(&'a str),
    Keyword(Name),
    Symbol(Name),
    List(&'a [Value]),
    Vector(&'a [Value]),
    /// A sorted map or a hash map.
    Map(MapRef<'a>),
    Bytes(&'a [u8]),
    /// Any other value: procedures, promises, records, bignums, rationals,
    /// streams, typed arrays, and future variants.
    Other(&'a Value),
}

impl<'a> ValueRef<'a> {
    /// View one value.
    pub fn of(value: &'a Value) -> Self {
        match value.view_ref() {
            ValueViewRef::Nil => Self::Nil,
            ValueViewRef::Bool(b) => Self::Bool(b),
            ValueViewRef::Int(n) => Self::Int(n),
            ValueViewRef::Float(f) => Self::Float(f),
            ValueViewRef::Char(c) => Self::Char(c),
            ValueViewRef::String(s) => Self::String(s),
            ValueViewRef::Keyword(spur) => Self::Keyword(Name::new(spur)),
            ValueViewRef::Symbol(spur) => Self::Symbol(Name::new(spur)),
            ValueViewRef::List(items) => Self::List(items),
            ValueViewRef::Vector(items) => Self::Vector(items),
            ValueViewRef::Map(map) => Self::Map(MapRef::Sorted(map)),
            ValueViewRef::HashMap(map) => Self::Map(MapRef::Hash(map)),
            ValueViewRef::Bytevector(bytes) => Self::Bytes(bytes),
            _ => Self::Other(value),
        }
    }

    /// View every value in an argument slice, for slice-pattern matching.
    pub fn slice(values: &'a [Value]) -> Vec<Self> {
        values.iter().map(Self::of).collect()
    }

    /// The Sema type name, as reported by `type` in Sema code.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Char(_) => "char",
            Self::String(_) => "string",
            Self::Keyword(_) => "keyword",
            Self::Symbol(_) => "symbol",
            Self::List(_) => "list",
            Self::Vector(_) => "vector",
            Self::Map(MapRef::Sorted(_)) => "map",
            Self::Map(MapRef::Hash(_)) => "hashmap",
            Self::Bytes(_) => "bytevector",
            Self::Other(value) => value.type_name(),
        }
    }
}

/// The name of a keyword or symbol. Compares with `&str` and displays as the
/// name without the `:` prefix.
///
/// A name is a key into the current thread's string interner, so it is
/// neither `Send` nor `Sync`. Convert it with [`Name::to_string`] before
/// sending it to another thread:
///
/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// assert_send::<sema_core::Name>();
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Name {
    spur: Spur,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Name {
    fn new(spur: Spur) -> Self {
        Self {
            spur,
            _thread_bound: PhantomData,
        }
    }

    /// Run `f` with the name as a `&str`, without allocating.
    pub fn with_str<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        with_resolved(self.spur, f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.with_str(|s| f.write_str(s))
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.with_str(|s| write!(f, "Name({s:?})"))
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.with_str(|s| s == other)
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.with_str(|s| s == *other)
    }
}

/// A borrowed Sema map: either a sorted `{…}` map or a hash map.
#[derive(Clone, Copy)]
pub enum MapRef<'a> {
    Sorted(&'a BTreeMap<Value, Value>),
    Hash(&'a hashbrown::HashMap<Value, Value>),
}

impl<'a> MapRef<'a> {
    pub fn len(&self) -> usize {
        match self {
            Self::Sorted(map) => map.len(),
            Self::Hash(map) => map.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, key: &Value) -> Option<&'a Value> {
        match self {
            Self::Sorted(map) => map.get(key),
            Self::Hash(map) => map.get(key),
        }
    }

    /// Look up a keyword key by name: `map.get_keyword("status")` reads `:status`.
    pub fn get_keyword(&self, name: &str) -> Option<&'a Value> {
        self.get(&Value::keyword(name))
    }

    /// Iterate entries. Sorted maps iterate in key order; hash maps in no
    /// particular order.
    pub fn iter(&self) -> Box<dyn Iterator<Item = (&'a Value, &'a Value)> + 'a> {
        match *self {
            Self::Sorted(map) => Box::new(map.iter()),
            Self::Hash(map) => Box::new(map.iter()),
        }
    }
}

impl fmt::Debug for MapRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_common_values() {
        let s = Value::string("hi");
        assert!(matches!(ValueRef::of(&Value::nil()), ValueRef::Nil));
        assert!(matches!(ValueRef::of(&Value::int(7)), ValueRef::Int(7)));
        assert!(matches!(ValueRef::of(&s), ValueRef::String("hi")));
        let kw = Value::keyword("status");
        match ValueRef::of(&kw) {
            ValueRef::Keyword(name) => {
                assert_eq!(name, "status");
                assert_eq!(name.to_string(), "status");
            }
            other => panic!("expected keyword, got {other:?}"),
        }
    }

    #[test]
    fn matches_argument_slices() {
        let args = [Value::keyword("retry"), Value::int(3)];
        let matched = match ValueRef::slice(&args).as_slice() {
            [ValueRef::Keyword(k), ValueRef::Int(n)] if *k == "retry" => *n,
            _ => -1,
        };
        assert_eq!(matched, 3);
    }

    #[test]
    fn unknown_values_are_other() {
        let f = Value::native_fn(crate::NativeFn::simple("f", |_| Ok(Value::nil())));
        match ValueRef::of(&f) {
            ValueRef::Other(v) => assert_eq!(v.type_name(), f.type_name()),
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
