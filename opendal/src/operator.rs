//! Operator lifecycle across the FFI boundary.
//!
//! `od_operator_new` builds an `opendal::Operator` from a **URI**
//! (`scheme://authority`) plus a flat key/value config array. It delegates to
//! `Operator::from_uri`, so OpenDAL's per-service URI parsing decides how the
//! authority maps to config (e.g. s3 authority → `bucket`, azblob → `container`,
//! fs/memory → no authority). The extra key/value pairs are OpenDAL config
//! (from a SCOPE-matched secret) and **override** anything parsed from the URI.
//! The handle is opaque to C++; free it with `od_operator_free`.
//! Reader/stat/etc. borrow the operator, so it must outlive all handles derived
//! from it.

use std::ffi::c_char;

use opendal::{Operator, OperatorUri};

use crate::error::{set_error, set_ok, set_opendal_error, OdError, OdErrorCode};
use crate::ffi::{cstr, ffi_guard, free_handle};
use crate::layers::apply_layers;

/// Opaque handle wrapping an `opendal::Operator`.
pub struct OdOperator {
    pub(crate) op: Operator,
    /// The scheme this operator was built for (e.g. "s3"). Used to produce
    /// clear "service '<scheme>' does not support <op>" capability errors.
    pub(crate) scheme: String,
    /// Reader/writer I/O tuning (concurrent + chunk), resolved from per-operator
    /// `io.*` options merged over the process-global defaults.
    pub(crate) io: crate::io::IoOptions,
    /// The operator's capability, cached at construction. It is immutable for
    /// the operator's lifetime, so the fail-fast guards read it from here
    /// instead of re-deriving it (which clones the Arc-backed ServiceInfo) on
    /// every read/write/stat/list.
    pub(crate) cap: opendal::Capability,
}

/// Build an Operator from `uri` (`scheme://authority`) plus `len` key/value
/// config pairs, then apply optional layers described by `layer_len` pairs.
///
/// Construction goes through `Operator::from_uri((uri, extra_opts))`: OpenDAL's
/// per-service URI parsing maps the authority to the right config key (bucket /
/// container / …); `keys`/`values` are extra OpenDAL config (typically a
/// SCOPE-matched secret) that override URI-parsed values.
/// `layer_keys`/`layer_values` configure layers (retry/timeout/… — see
/// `layers.rs`).
/// On success returns a non-null `*mut OdOperator` and sets `*err` to Ok.
/// On failure returns null and populates `*err`.
///
/// # Safety
/// - `uri`, and each config/layer `keys[i]`/`values[i]`, must be valid
///   NUL-terminated C strings for the duration of the call.
/// - `keys`/`values` must each point to `len` valid pointers (or be null iff
///   `len == 0`); likewise `layer_keys`/`layer_values` and `layer_len`.
/// - The returned handle must be freed exactly once with `od_operator_free`.
#[no_mangle]
pub unsafe extern "C" fn od_operator_new(
    uri: *const c_char,
    keys: *const *const c_char,
    values: *const *const c_char,
    len: usize,
    layer_keys: *const *const c_char,
    layer_values: *const *const c_char,
    layer_len: usize,
    err: *mut OdError,
) -> *mut OdOperator {
    ffi_guard!(err, std::ptr::null_mut(), "od_operator_new", {
        let uri_str = match cstr(uri) {
            Some(s) => s,
            None => {
                set_error(err, OdErrorCode::InvalidInput, "uri is null or not UTF-8");
                return std::ptr::null_mut();
            }
        };

        // Extra config (secret) + layer options from the parallel arrays.
        let cfg = match collect_pairs(keys, values, len) {
            Ok(v) => v,
            Err(msg) => {
                set_error(err, OdErrorCode::InvalidInput, msg);
                return std::ptr::null_mut();
            }
        };
        let layer_opts = match collect_pairs(layer_keys, layer_values, layer_len) {
            Ok(v) => v,
            Err(msg) => {
                set_error(err, OdErrorCode::InvalidInput, msg);
                return std::ptr::null_mut();
            }
        };

        // Parse the URI once (folding the secret config in as extra options),
        // then reuse it for both the scheme and operator construction.
        let parsed = match OperatorUri::new(uri_str, cfg) {
            Ok(p) => p,
            Err(e) => {
                set_opendal_error(err, &e);
                return std::ptr::null_mut();
            }
        };
        let scheme = parsed.scheme().to_owned();

        // Resolve I/O tuning: per-operator io.* options over global defaults.
        let io = crate::io::IoOptions::from_opts(&layer_opts).with_defaults(&crate::io::global());

        match Operator::from_uri(parsed) {
            Ok(op) => {
                let op = apply_layers(op, &layer_opts);
                let cap = op.info().capability();
                set_ok(err);
                Box::into_raw(Box::new(OdOperator {
                    op,
                    scheme,
                    io,
                    cap,
                }))
            }
            Err(e) => {
                set_opendal_error(err, &e);
                std::ptr::null_mut()
            }
        }
    })
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
/// `op` must be null or a handle from `od_operator_new`, not already freed,
/// with no live Reader/Lister/etc. still borrowing it.
#[no_mangle]
pub unsafe extern "C" fn od_operator_free(op: *mut OdOperator) {
    free_handle(op);
}

/// Whether `scheme` is a service compiled into this build (i.e. registered in
/// OpenDAL's operator registry), so we don't hardcode the supported set.
///
/// Probes `Operator::via_iter(scheme, [])`: OpenDAL resolves the scheme through
/// the registry with no config, without I/O. `ErrorKind::Unsupported` ("scheme
/// is not registered") means the service was not compiled in; `Ok` or any other
/// error (e.g. a missing-config error) means it IS registered. Results are
/// cached — the registered set is fixed for the process. Returns 1/0.
///
/// Requires `od_init()` to have populated the registry first.
///
/// # Safety
/// `scheme` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn od_scheme_supported(scheme: *const c_char) -> u8 {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let s = match cstr(scheme) {
            Some(s) => s,
            None => return 0u8,
        };
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(&hit) = cache.lock().unwrap().get(s) {
            return hit as u8;
        }
        let supported = match Operator::via_iter(s, []) {
            Ok(_) => true,
            Err(e) => e.kind() != opendal::ErrorKind::Unsupported,
        };
        cache.lock().unwrap().insert(s.to_owned(), supported);
        supported as u8
    }));
    result.unwrap_or(0)
}
