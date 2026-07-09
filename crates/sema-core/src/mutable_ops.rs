//! Shared implementations of the `mutable-array` accessors.
//!
//! Both the stdlib natives (`sema-stdlib/src/mutable.rs`) and the VM's
//! `MutArrGet` / `MutArrSet` intrinsic opcodes (`sema-vm/src/vm.rs`) dispatch
//! into these, so the two paths raise byte-identical errors and can never
//! drift apart semantically.

use crate::{SemaError, Value};

fn not_an_array(v: &Value, name: &str) -> SemaError {
    SemaError::type_error("mutable-array", v.type_name())
        .with_hint(format!("{name}: create one with (mutable-array/new)"))
}

/// `mutable-array/get`: indexed read. `default` is the optional third
/// argument — returned on an out-of-bounds index instead of erroring.
/// Type and index errors are raised regardless of the default.
pub fn mutable_array_get(
    arr: &Value,
    idx: &Value,
    default: Option<&Value>,
) -> Result<Value, SemaError> {
    let a = arr
        .as_mutable_array()
        .ok_or_else(|| not_an_array(arr, "mutable-array/get"))?;
    let idx = idx.as_index("mutable-array/get")?;
    let items = a.items.borrow();
    match items.get(idx) {
        Some(v) => Ok(v.clone()),
        None => match default {
            Some(d) => Ok(d.clone()),
            None => Err(SemaError::eval(format!(
                "mutable-array/get: index {idx} out of bounds (length {})",
                items.len()
            ))),
        },
    }
}

/// `mutable-array/set!`: indexed write into an existing slot. Takes `val` by
/// move so a caller that owns the value (the VM's `MutArrSet` arm) pays no
/// clone. The Sema-level contract returns the array itself — the caller hands
/// back its own array handle after `Ok(())`.
pub fn mutable_array_set(arr: &Value, idx: &Value, val: Value) -> Result<(), SemaError> {
    let a = arr
        .as_mutable_array()
        .ok_or_else(|| not_an_array(arr, "mutable-array/set!"))?;
    let idx = idx.as_index("mutable-array/set!")?;
    let mut items = a.items.borrow_mut();
    let len = items.len();
    match items.get_mut(idx) {
        Some(slot) => {
            *slot = val;
            Ok(())
        }
        None => Err(SemaError::eval(format!(
            "mutable-array/set!: index {idx} out of bounds (length {len})"
        ))
        .with_hint("set! writes to an existing slot; use mutable-array/push! to grow")),
    }
}
