//! Capability introspection + fail-fast guards.
//!
//! OpenDAL exposes each service's supported operations via
//! `Operator::info().capability()` (a cheap, cached `Capability` of `bool`
//! flags — the effective capability after layers/simulation, per RFC 7700). We use it two ways:
//!
//!   1. **Fail-fast guards** — before a mutating/IO call we check the relevant
//!      flag and, if unsupported, return `OdErrorCode::Unsupported` with a
//!      clear "service '<scheme>' does not support <op>" message, rather than
//!      letting the deep OpenDAL call fail with a less obvious error.
//!   2. **Introspection** — `od_capabilities` materializes every boolean
//!      capability as an index-addressable `(name, supported)` list so the C++
//!      side can surface them generically (e.g. a future `opendal_fs_services()`)
//!      without hardcoding a column per flag.

use std::ffi::{c_char, CString};

use opendal::Capability;

use crate::error::{set_error, OdError, OdErrorCode};
use crate::ffi::{ffi_guard, free_handle};
use crate::operator::OdOperator;

/// Enumerate an operator's capabilities as `(name, supported)` pairs.
///
/// `opendal::Capability` derives `Serialize`, so we serialize it to a JSON
/// object and keep every boolean field — no hand-maintained field list, so this
/// automatically tracks capabilities added in future OpenDAL versions. The
/// non-boolean size-hint fields (`write_multi_max_size`, …) are skipped: they
/// are limits, not "supported / not-supported" flags. Field order is whatever
/// serde_json yields (deterministic per build); the C++ side treats it as a set.
fn capability_bools(c: &Capability) -> Vec<(String, bool)> {
    match serde_json::to_value(c) {
        Ok(serde_json::Value::Object(map)) => map
            .into_iter()
            .filter_map(|(k, v)| v.as_bool().map(|b| (k, b)))
            .collect(),
        _ => Vec::new(),
    }
}

/// Fail-fast guard: return `Err((code, message))` if `supported` is false.
///
/// `supported` is the relevant `Capability` flag; `op_name` names the operation
/// for the error message. Call from a `catch_unwind`-guarded FFI body and set
/// the error out-param from the returned tuple.
pub(crate) fn require(
    scheme: &str,
    supported: bool,
    op_name: &str,
) -> Result<(), (OdErrorCode, String)> {
    if supported {
        Ok(())
    } else {
        Err((
            OdErrorCode::Unsupported,
            format!("service '{scheme}' does not support {op_name}"),
        ))
    }
}

// ── introspection FFI: capabilities as an index-addressable (name, bool) list ─

/// One capability flag: `name` (borrowed, NUL-terminated) + `supported`.
#[repr(C)]
pub struct OdCapability {
    /// Capability name (e.g. "write_can_append"). Borrowed from the list; do
    /// NOT free. Valid until `od_capabilities_free`.
    pub name: *const c_char,
    /// 1 if supported, 0 otherwise.
    pub supported: u8,
}

/// Opaque, index-addressable list of an operator's boolean capabilities.
pub struct OdCapabilityList {
    items: Vec<(CString, bool)>,
}

/// Materialize every boolean capability of `op` into an index-addressable list.
///
/// Returns null only on null input or panic. Free with
/// `od_capabilities_free`.
///
/// # Safety
/// `op` must be a live handle from `od_operator_new`. `err` must be valid.
#[no_mangle]
pub unsafe extern "C" fn od_capabilities(
    op: *const OdOperator,
    err: *mut OdError,
) -> *mut OdCapabilityList {
    ffi_guard!(err, std::ptr::null_mut(), "od_capabilities", {
        if op.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null operator");
            return std::ptr::null_mut();
        }
        let cap = (*op).cap;
        let items: Vec<(CString, bool)> = capability_bools(&cap)
            .into_iter()
            .map(|(name, sup)| (CString::new(name).unwrap_or_default(), sup))
            .collect();
        crate::error::set_ok(err);
        Box::into_raw(Box::new(OdCapabilityList { items }))
    })
}

/// Number of capability entries in the list. 0 on null.
///
/// # Safety
/// `list` must be null or a handle from `od_capabilities`.
#[no_mangle]
pub unsafe extern "C" fn od_capabilities_len(list: *const OdCapabilityList) -> usize {
    if list.is_null() {
        return 0;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (*list).items.len())).unwrap_or(0)
}

/// Read capability `index` into `out`. Returns 1 on success, 0 on out-of-range
/// or null. `out.name` borrows from the list (valid until it is freed).
///
/// # Safety
/// `list` must be a live handle from `od_capabilities`; `out` must be valid.
#[no_mangle]
pub unsafe extern "C" fn od_capabilities_entry(
    list: *const OdCapabilityList,
    index: usize,
    out: *mut OdCapability,
) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if list.is_null() || out.is_null() {
            return 0u8;
        }
        let items = &(*list).items;
        if index >= items.len() {
            return 0;
        }
        let (name, supported) = &items[index];
        (*out).name = name.as_ptr();
        (*out).supported = if *supported { 1 } else { 0 };
        1
    }));
    result.unwrap_or(0)
}

/// Free a capability list. Safe with null (no-op).
///
/// # Safety
/// `list` must be null or a handle from `od_capabilities`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn od_capabilities_free(list: *mut OdCapabilityList) {
    free_handle(list);
}
