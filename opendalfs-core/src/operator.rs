//! Operator lifecycle across the FFI boundary.
//!
//! `odop_operator_new` builds an `opendal::Operator` from a scheme string and a
//! flat key/value config array (the same string→string map OpenDAL accepts for
//! every service). The handle is opaque to C++; free it with
//! `odop_operator_free`. Reader/stat/etc. borrow the operator, so it must
//! outlive all handles derived from it.

use std::ffi::{c_char, CStr};

use opendal::Operator;

use crate::error::{set_error, set_ok, set_opendal_error, OdopError, OdopErrorCode};
use crate::layers::apply_layers;

/// Opaque handle wrapping an `opendal::Operator`.
pub struct OdopOperator {
    pub(crate) op: Operator,
    /// The scheme this operator was built for (e.g. "s3"). Used to produce
    /// clear "service '<scheme>' does not support <op>" capability errors.
    pub(crate) scheme: String,
}

/// Read a `*const c_char` into a Rust `&str`, returning None on null/invalid UTF-8.
unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

/// Build an Operator for `scheme` configured by `len` key/value pairs, then
/// apply optional layers described by `layer_len` key/value pairs.
///
/// `keys`/`values` are the OpenDAL service config; `layer_keys`/`layer_values`
/// configure layers (retry/timeout/concurrent-limit — see `layers.rs`).
/// On success returns a non-null `*mut OdopOperator` and sets `*err` to Ok.
/// On failure returns null and populates `*err`.
///
/// # Safety
/// - `scheme`, and each config/layer `keys[i]`/`values[i]`, must be valid
///   NUL-terminated C strings for the duration of the call.
/// - `keys`/`values` must each point to `len` valid pointers (or be null iff
///   `len == 0`); likewise `layer_keys`/`layer_values` and `layer_len`.
/// - The returned handle must be freed exactly once with `odop_operator_free`.
#[no_mangle]
pub unsafe extern "C" fn odop_operator_new(
    scheme: *const c_char,
    keys: *const *const c_char,
    values: *const *const c_char,
    len: usize,
    layer_keys: *const *const c_char,
    layer_values: *const *const c_char,
    layer_len: usize,
    err: *mut OdopError,
) -> *mut OdopOperator {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let scheme_str = match cstr(scheme) {
            Some(s) => s,
            None => {
                set_error(err, OdopErrorCode::InvalidInput, "scheme is null or not UTF-8");
                return std::ptr::null_mut();
            }
        };

        // Collect the config map from the parallel arrays.
        let cfg = match collect_pairs(keys, values, len) {
            Ok(v) => v,
            Err(msg) => {
                set_error(err, OdopErrorCode::InvalidInput, msg);
                return std::ptr::null_mut();
            }
        };

        // Collect layer options.
        let layer_opts = match collect_pairs(layer_keys, layer_values, layer_len) {
            Ok(v) => v,
            Err(msg) => {
                set_error(err, OdopErrorCode::InvalidInput, msg);
                return std::ptr::null_mut();
            }
        };

        match Operator::via_iter(scheme_str, cfg) {
            Ok(op) => {
                let op = apply_layers(op, &layer_opts);
                set_ok(err);
                Box::into_raw(Box::new(OdopOperator {
                    op,
                    scheme: scheme_str.to_owned(),
                }))
            }
            Err(e) => {
                set_opendal_error(err, &e);
                std::ptr::null_mut()
            }
        }
    }));

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            set_error(err, OdopErrorCode::Panic, "panic in odop_operator_new");
            std::ptr::null_mut()
        }
    }
}

/// Collect `len` parallel (key, value) C-string pairs into owned Rust strings.
unsafe fn collect_pairs(
    keys: *const *const c_char,
    values: *const *const c_char,
    len: usize,
) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(len);
    if len == 0 {
        return Ok(out);
    }
    if keys.is_null() || values.is_null() {
        return Err("keys/values null with len > 0".to_string());
    }
    let key_slice = std::slice::from_raw_parts(keys, len);
    let val_slice = std::slice::from_raw_parts(values, len);
    for i in 0..len {
        let k = cstr(key_slice[i]).ok_or_else(|| format!("key #{i} invalid"))?;
        let v = cstr(val_slice[i]).ok_or_else(|| format!("value for '{k}' invalid"))?;
        out.push((k.to_owned(), v.to_owned()));
    }
    Ok(out)
}

/// Free an operator handle. Safe to call with null (no-op).
///
/// # Safety
/// `op` must be null or a handle from `odop_operator_new`, not already freed,
/// with no live Reader/Lister/etc. still borrowing it.
#[no_mangle]
pub unsafe extern "C" fn odop_operator_free(op: *mut OdopOperator) {
    if op.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(Box::from_raw(op));
    }));
}
