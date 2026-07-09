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

/// Opaque handle wrapping an `opendal::Operator`.
pub struct OdopOperator {
    pub(crate) op: Operator,
}

/// Read a `*const c_char` into a Rust `&str`, returning None on null/invalid UTF-8.
unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

/// Build an Operator for `scheme` configured by `len` key/value pairs.
///
/// `keys` and `values` are parallel arrays of C strings, each of length `len`.
/// On success returns a non-null `*mut OdopOperator` and sets `*err` to Ok.
/// On failure returns null and populates `*err`.
///
/// # Safety
/// - `scheme`, and each `keys[i]`/`values[i]`, must be valid NUL-terminated C
///   strings for the duration of the call.
/// - `keys`/`values` must each point to `len` valid pointers (or be null iff
///   `len == 0`).
/// - The returned handle must be freed exactly once with `odop_operator_free`.
#[no_mangle]
pub unsafe extern "C" fn odop_operator_new(
    scheme: *const c_char,
    keys: *const *const c_char,
    values: *const *const c_char,
    len: usize,
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
        let mut cfg: Vec<(String, String)> = Vec::with_capacity(len);
        if len > 0 {
            if keys.is_null() || values.is_null() {
                set_error(err, OdopErrorCode::InvalidInput, "keys/values null with len > 0");
                return std::ptr::null_mut();
            }
            let key_slice = std::slice::from_raw_parts(keys, len);
            let val_slice = std::slice::from_raw_parts(values, len);
            for i in 0..len {
                let k = match cstr(key_slice[i]) {
                    Some(s) => s.to_owned(),
                    None => {
                        set_error(err, OdopErrorCode::InvalidInput, format!("config key #{i} invalid"));
                        return std::ptr::null_mut();
                    }
                };
                let v = match cstr(val_slice[i]) {
                    Some(s) => s.to_owned(),
                    None => {
                        set_error(err, OdopErrorCode::InvalidInput, format!("config value for '{k}' invalid"));
                        return std::ptr::null_mut();
                    }
                };
                cfg.push((k, v));
            }
        }

        match Operator::via_iter(scheme_str, cfg) {
            Ok(op) => {
                set_ok(err);
                Box::into_raw(Box::new(OdopOperator { op }))
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
