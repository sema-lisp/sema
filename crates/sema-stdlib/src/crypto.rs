//! Hashing, base64, and UUID builtins.
//!
//! **Bounded synchronous CPU (B9 R21 split).** Every hash (`hash/md5`,
//! `hash/sha256`, `hash/hmac-sha256`) and base64 transform here is a plain
//! O(input) pass with no cost parameter to tune — there is nothing to offload, so
//! they stay SYNCHRONOUS. Inside a runtime quantum (`in_runtime_quantum()`) each
//! captures a pre-dispatch input-byte cap so its VM-thread CPU is bounded (bounded
//! input ⇒ bounded work) — an explicit synchronous split, not a fake async wrap. A
//! direct native call outside the cooperative runtime keeps the uncapped shape.
//! `uuid/v4` takes no input, so it needs no cap.

use hmac::{Hmac, Mac};
use sema_core::{check_arity, SemaError, Value};
use sha2::{Digest, Sha256};

use crate::register_fn;

type HmacSha256 = Hmac<Sha256>;

/// Per-input byte cap for the hashing/base64 ops under a runtime quantum. These
/// are cheap O(input) passes; 64 MiB is far above any realistic hash/encode input.
#[cfg(not(target_arch = "wasm32"))]
const CRYPTO_INPUT_BYTE_CAP: u64 = 64 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// Optional per-call input-byte cap override (lowered, never raised above the
    /// hard ceiling). Read on the VM thread; mirrors
    /// `diff::DIFF_INPUT_BYTE_CAP_OVERRIDE`. `None` uses the module ceiling. The
    /// seam the regression suite drives to exercise the cap boundary without a
    /// multi-megabyte input string.
    static CRYPTO_INPUT_BYTE_CAP_OVERRIDE: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

/// The effective per-input byte cap for the current call: the module ceiling,
/// lowered by any per-call override (never raised above it).
#[cfg(not(target_arch = "wasm32"))]
fn effective_crypto_input_byte_cap() -> u64 {
    CRYPTO_INPUT_BYTE_CAP_OVERRIDE
        .with(std::cell::Cell::get)
        .map_or(CRYPTO_INPUT_BYTE_CAP, |over| {
            over.min(CRYPTO_INPUT_BYTE_CAP)
        })
}

/// Lower the per-input byte cap (clamped to the hard ceiling) for subsequent
/// hashing/base64 calls on this thread, or clear the override with `None`. Test
/// seam, mirroring `set_diff_input_byte_cap_override`.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_crypto_input_byte_cap_override(bytes: Option<u64>) {
    CRYPTO_INPUT_BYTE_CAP_OVERRIDE.with(|cell| cell.set(bytes));
}

/// Reject `actual` bytes over `limit`. Reads the argument's existing `len()` — no
/// snapshot — so an over-cap input is rejected without any excess allocation.
#[cfg(not(target_arch = "wasm32"))]
fn check_crypto_limit(op: &str, actual: u64, limit: u64) -> Result<(), SemaError> {
    if actual > limit {
        return Err(SemaError::eval(format!(
            "{op}: input bytes {actual} exceeds the quarantined limit {limit}"
        ))
        .with_hint("reduce or split the input"));
    }
    Ok(())
}

/// Enforce the input-byte cap for `op` ONLY inside a runtime quantum (a direct
/// native call keeps the uncapped shape). No-op on wasm (no cooperative runtime).
fn crypto_cap(op: &str, len: usize) -> Result<(), SemaError> {
    #[cfg(not(target_arch = "wasm32"))]
    if sema_core::in_runtime_quantum() {
        return check_crypto_limit(op, len as u64, effective_crypto_input_byte_cap());
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (op, len);
    Ok(())
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "uuid/v4", |args| {
        check_arity!(args, "uuid/v4", 0);
        Ok(Value::string(&uuid::Uuid::new_v4().to_string()))
    });

    register_fn(env, "base64/encode", |args| {
        check_arity!(args, "base64/encode", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        crypto_cap("base64/encode", s.len())?;
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
        Ok(Value::string(&encoded))
    });

    register_fn(env, "base64/decode", |args| {
        check_arity!(args, "base64/decode", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        crypto_cap("base64/decode", s.len())?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(|e| SemaError::eval(format!("base64/decode: {e}")))?;
        let decoded = String::from_utf8(bytes)
            .map_err(|e| SemaError::eval(format!("base64/decode: invalid UTF-8: {e}")))?;
        Ok(Value::string(&decoded))
    });

    register_fn(env, "base64/encode-bytes", |args| {
        check_arity!(args, "base64/encode-bytes", 1);
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        crypto_cap("base64/encode-bytes", bv.len())?;
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bv);
        Ok(Value::string(&encoded))
    });

    register_fn(env, "base64/decode-bytes", |args| {
        check_arity!(args, "base64/decode-bytes", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        crypto_cap("base64/decode-bytes", s.len())?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(|e| SemaError::eval(format!("base64/decode-bytes: {e}")))?;
        Ok(Value::bytevector(bytes))
    });

    register_fn(env, "hash/md5", |args| {
        check_arity!(args, "hash/md5", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        crypto_cap("hash/md5", s.len())?;
        let digest = md5::compute(s.as_bytes());
        Ok(Value::string(&format!("{:x}", digest)))
    });

    register_fn(env, "hash/hmac-sha256", |args| {
        check_arity!(args, "hash/hmac-sha256", 2);
        let key = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let message = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        crypto_cap("hash/hmac-sha256", key.len().saturating_add(message.len()))?;
        let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(Value::string(&hex::encode(result.into_bytes())))
    });

    register_fn(env, "hash/sha256", |args| {
        check_arity!(args, "hash/sha256", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        crypto_cap("hash/sha256", s.len())?;
        let hash = Sha256::digest(s.as_bytes());
        let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
        Ok(Value::string(&hex))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{intern, Env, EvalContext};

    fn call(env: &Env, name: &str, args: &[Value]) -> Result<Value, SemaError> {
        let f = env
            .get(intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        let nf = f.as_native_fn_ref().expect("native fn");
        let ctx = EvalContext::default();
        (nf.func)(&ctx, args)
    }

    #[test]
    fn sha256_and_base64_round_trip_uncapped_outside_a_quantum() {
        let env = Env::new();
        register(&env);
        // A direct native call (no runtime quantum) is never capped, even below a
        // lowered override.
        #[cfg(not(target_arch = "wasm32"))]
        set_crypto_input_byte_cap_override(Some(1));
        let sha = call(&env, "hash/sha256", &[Value::string("")]).unwrap();
        assert_eq!(
            sha.as_str().unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let enc = call(&env, "base64/encode", &[Value::string("hello")]).unwrap();
        assert_eq!(enc.as_str().unwrap(), "aGVsbG8=");
        #[cfg(not(target_arch = "wasm32"))]
        set_crypto_input_byte_cap_override(None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn crypto_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_crypto_limit("hash/sha256", 8, 8).is_ok());
        let error = check_crypto_limit("hash/sha256", 9, 8)
            .expect_err("one byte over the captured limit must fail");
        assert!(error.to_string().contains('9'));
        assert!(error.to_string().contains('8'));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn crypto_input_byte_cap_is_finite_and_clamps_overrides() {
        assert_eq!(effective_crypto_input_byte_cap(), CRYPTO_INPUT_BYTE_CAP);
        set_crypto_input_byte_cap_override(Some(16));
        assert_eq!(effective_crypto_input_byte_cap(), 16);
        // An override above the hard ceiling is clamped down, never raised.
        set_crypto_input_byte_cap_override(Some(u64::MAX));
        assert_eq!(effective_crypto_input_byte_cap(), CRYPTO_INPUT_BYTE_CAP);
        set_crypto_input_byte_cap_override(None);
        assert_eq!(effective_crypto_input_byte_cap(), CRYPTO_INPUT_BYTE_CAP);
    }
}
