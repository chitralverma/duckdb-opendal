//! Operator lifecycle across the FFI boundary.
//!
//! `od_operator_new` builds an `opendal::Operator` from a **URI**
//! (`scheme://authority`) plus a flat key/value config array. URL path remains
//! separate as the operation path. OpenDAL's service configurator interprets
//! authority; all other service configuration comes from the scoped secret.
//! The handle is opaque to C++; free it with `od_operator_free`.
//! Reader/stat/etc. borrow the operator, so it must outlive all handles derived
//! from it.

use std::collections::HashSet;
use std::ffi::{c_char, CString};
use std::sync::OnceLock;

use opendal::{Operator, OperatorUri};

use crate::config::OperatorConfig;
use crate::error::{set_error, set_ok, set_opendal_error, OdError, OdErrorCode};
use crate::ffi::{cstr, ffi_guard, free_handle};

/// Opaque handle wrapping an `opendal::Operator`.
pub struct OdOperator {
    pub(crate) op: Operator,
    /// The scheme this operator was built for (e.g. "s3"). Used to produce
    /// clear "service '<scheme>' does not support <op>" capability errors.
    pub(crate) scheme: String,
    /// Reader/writer I/O tuning (concurrent + chunk), resolved from per-operator
    /// `io.*` options merged over the process-global defaults.
    pub(crate) io: crate::config::IoConfig,
    /// The operator's capability, cached at construction. It is immutable for
    /// the operator's lifetime, so the fail-fast guards read it from here
    /// instead of re-deriving it (which clones the Arc-backed ServiceInfo) on
    /// every read/write/stat/list.
    pub(crate) cap: opendal::Capability,
    pub(crate) warning: Option<std::ffi::CString>,
}

/// Build an Operator from `uri` (`scheme://authority`) plus `len` key/value
/// config pairs, then parse cross-service options described by `option_len` pairs.
///
/// Construction goes through `Operator::from_uri((uri, extra_opts))`: OpenDAL's
/// per-service URI parsing maps the authority to the right config key (bucket /
/// container / …); `keys`/`values` are extra OpenDAL config (typically a
/// SCOPE-matched secret) that override URI-parsed values.
/// `option_keys`/`option_values` configure typed I/O/retry/timeout/cache sections.
/// On success returns a non-null `*mut OdOperator` and sets `*err` to Ok.
/// On failure returns null and populates `*err`.
///
/// # Safety
/// - `uri`, and each config/option `keys[i]`/`values[i]`, must be valid
///   NUL-terminated C strings for the duration of the call.
/// - `keys`/`values` must each point to `len` valid pointers (or be null iff
///   `len == 0`); likewise `option_keys`/`option_values` and `option_len`.
/// - The returned handle must be freed exactly once with `od_operator_free`.
#[no_mangle]
pub unsafe extern "C" fn od_operator_new(
    uri: *const c_char,
    keys: *const *const c_char,
    values: *const *const c_char,
    len: usize,
    option_keys: *const *const c_char,
    option_values: *const *const c_char,
    option_len: usize,
    cache_namespace: *const c_char,
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

        // Service config + cross-service options from parallel arrays.
        let cfg = match collect_pairs(keys, values, len) {
            Ok(v) => v,
            Err(msg) => {
                set_error(err, OdErrorCode::InvalidInput, msg);
                return std::ptr::null_mut();
            }
        };
        let options = match collect_pairs(option_keys, option_values, option_len) {
            Ok(v) => v,
            Err(msg) => {
                set_error(err, OdErrorCode::InvalidInput, msg);
                return std::ptr::null_mut();
            }
        };
        let cache_namespace = cstr(cache_namespace).map(ToOwned::to_owned);
        let config = match OperatorConfig::parse(options, cache_namespace) {
            Ok(config) => config,
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

        match Operator::from_uri(parsed) {
            Ok(op) => {
                let applied = config.apply_layers(op);
                let op = applied.op;
                let cap = op.info().capability();
                set_ok(err);
                Box::into_raw(Box::new(OdOperator {
                    op,
                    scheme,
                    io: config.io,
                    cap,
                    warning: applied
                        .warning
                        .and_then(|warning| std::ffi::CString::new(warning).ok()),
                }))
            }
            Err(e) => {
                set_opendal_error(err, &e);
                std::ptr::null_mut()
            }
        }
    })
}

/// Return an operator-construction warning, or null. Borrowed from `op`.
#[no_mangle]
pub unsafe extern "C" fn od_operator_warning(op: *const OdOperator) -> *const c_char {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if op.is_null() {
            std::ptr::null()
        } else {
            (*op)
                .warning
                .as_ref()
                .map_or(std::ptr::null(), |warning| warning.as_ptr())
        }
    }))
    .unwrap_or(std::ptr::null())
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

static REGISTERED_SCHEMES: OnceLock<(HashSet<String>, Vec<String>)> = OnceLock::new();

fn registered_schemes() -> &'static (HashSet<String>, Vec<String>) {
    // Static-library consumers cannot rely on OpenDAL's ctor registration.
    opendal::init_default_registry();
    REGISTERED_SCHEMES.get_or_init(|| {
        let set = opendal::OperatorRegistry::get().schemes();
        let mut sorted: Vec<_> = set.iter().cloned().collect();
        sorted.sort();
        (set, sorted)
    })
}

/// Whether `scheme` is a service compiled into this build (i.e. registered in
/// OpenDAL's operator registry), so we don't hardcode the supported set.
/// Initializes the default registry on first use. Returns 1/0.
///
/// # Safety
/// `scheme` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn od_scheme_supported(scheme: *const c_char) -> u8 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match cstr(scheme) {
        Some(s) => registered_schemes().0.contains(s) as u8,
        None => 0,
    }))
    .unwrap_or(0)
}

pub struct OdSchemeList {
    schemes: Vec<CString>,
}

/// Return every scheme registered in OpenDAL's global operator registry.
///
/// # Safety
/// `err` must be null or a valid, writable pointer.
#[no_mangle]
pub unsafe extern "C" fn od_schemes(err: *mut OdError) -> *mut OdSchemeList {
    ffi_guard!(err, std::ptr::null_mut(), "od_schemes", {
        let (_, sorted) = registered_schemes();
        let schemes = match sorted
            .iter()
            .cloned()
            .map(CString::new)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(schemes) => schemes,
            Err(_) => {
                set_error(
                    err,
                    OdErrorCode::Unexpected,
                    "registered scheme contains a NUL byte",
                );
                return std::ptr::null_mut();
            }
        };
        set_ok(err);
        Box::into_raw(Box::new(OdSchemeList { schemes }))
    })
}

/// Number of registered schemes. Returns 0 for null.
///
/// # Safety
/// `list` must be null or a live handle returned by `od_schemes`.
#[no_mangle]
pub unsafe extern "C" fn od_schemes_len(list: *const OdSchemeList) -> usize {
    if list.is_null() {
        0
    } else {
        std::panic::catch_unwind(|| (*list).schemes.len()).unwrap_or(0)
    }
}

/// Borrow registered scheme `index` until `od_schemes_free`.
///
/// # Safety
/// `list` must be a live handle returned by `od_schemes`.
#[no_mangle]
pub unsafe extern "C" fn od_schemes_entry(
    list: *const OdSchemeList,
    index: usize,
) -> *const c_char {
    std::panic::catch_unwind(|| {
        if list.is_null() {
            return std::ptr::null();
        }
        (&(*list).schemes)
            .get(index)
            .map_or(std::ptr::null(), |scheme| scheme.as_ptr())
    })
    .unwrap_or(std::ptr::null())
}

#[no_mangle]
/// Free a scheme list. Null is a no-op.
///
/// # Safety
/// `list` must be null or a live handle returned by `od_schemes`, not already freed.
pub unsafe extern "C" fn od_schemes_free(list: *mut OdSchemeList) {
    free_handle(list);
}
