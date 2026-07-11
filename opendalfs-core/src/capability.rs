//! Capability introspection + fail-fast guards.
//!
//! OpenDAL exposes each service's supported operations via
//! `Operator::info().full_capability()` (a cheap, cached `Capability` of `bool`
//! flags). We use it two ways:
//!
//!   1. **Fail-fast guards** — before a mutating/IO call we check the relevant
//!      flag and, if unsupported, return `OdopErrorCode::Unsupported` with a
//!      clear "service '<scheme>' does not support <op>" message, rather than
//!      letting the deep OpenDAL call fail with a less obvious error.
//!   2. **Introspection** — `odop_capabilities` materializes every boolean
//!      capability as an index-addressable `(name, supported)` list so the C++
//!      side can surface them generically (e.g. a future `opendal_fs_services()`)
//!      without hardcoding a column per flag.

use std::ffi::{c_char, CString};

use opendal::Capability;

use crate::error::{set_error, OdopError, OdopErrorCode};
use crate::operator::OdopOperator;

/// Every boolean field of `opendal::Capability`, paired with its name. This is
/// the single source of truth for both guards and introspection: adding a new
/// OpenDAL capability means adding one line here.
///
/// The `Option<usize>` size-hint fields (`write_multi_max_size`, …) are not
/// boolean "supported/not" flags and are intentionally omitted.
fn capability_bools(c: &Capability) -> [(&'static str, bool); 51] {
    [
        ("stat", c.stat),
        ("stat_with_if_match", c.stat_with_if_match),
        ("stat_with_if_none_match", c.stat_with_if_none_match),
        ("stat_with_if_modified_since", c.stat_with_if_modified_since),
        ("stat_with_if_unmodified_since", c.stat_with_if_unmodified_since),
        ("stat_with_override_cache_control", c.stat_with_override_cache_control),
        ("stat_with_override_content_disposition", c.stat_with_override_content_disposition),
        ("stat_with_override_content_type", c.stat_with_override_content_type),
        ("stat_with_version", c.stat_with_version),
        ("read", c.read),
        ("read_with_if_match", c.read_with_if_match),
        ("read_with_if_none_match", c.read_with_if_none_match),
        ("read_with_if_modified_since", c.read_with_if_modified_since),
        ("read_with_if_unmodified_since", c.read_with_if_unmodified_since),
        ("read_with_override_cache_control", c.read_with_override_cache_control),
        ("read_with_override_content_disposition", c.read_with_override_content_disposition),
        ("read_with_override_content_type", c.read_with_override_content_type),
        ("read_with_version", c.read_with_version),
        ("write", c.write),
        ("write_can_multi", c.write_can_multi),
        ("write_can_empty", c.write_can_empty),
        ("write_can_append", c.write_can_append),
        ("write_with_content_type", c.write_with_content_type),
        ("write_with_content_disposition", c.write_with_content_disposition),
        ("write_with_content_encoding", c.write_with_content_encoding),
        ("write_with_cache_control", c.write_with_cache_control),
        ("write_with_if_match", c.write_with_if_match),
        ("write_with_if_none_match", c.write_with_if_none_match),
        ("write_with_if_not_exists", c.write_with_if_not_exists),
        ("write_with_user_metadata", c.write_with_user_metadata),
        ("create_dir", c.create_dir),
        ("delete", c.delete),
        ("delete_with_version", c.delete_with_version),
        ("delete_with_recursive", c.delete_with_recursive),
        ("copy", c.copy),
        ("copy_with_if_not_exists", c.copy_with_if_not_exists),
        ("copy_with_if_match", c.copy_with_if_match),
        ("copy_can_multi", c.copy_can_multi),
        ("rename", c.rename),
        ("list", c.list),
        ("list_with_limit", c.list_with_limit),
        ("list_with_start_after", c.list_with_start_after),
        ("list_with_recursive", c.list_with_recursive),
        ("list_with_versions", c.list_with_versions),
        ("list_with_deleted", c.list_with_deleted),
        ("presign", c.presign),
        ("presign_read", c.presign_read),
        ("presign_stat", c.presign_stat),
        ("presign_write", c.presign_write),
        ("presign_delete", c.presign_delete),
        ("shared", c.shared),
    ]
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
) -> Result<(), (OdopErrorCode, String)> {
    if supported {
        Ok(())
    } else {
        Err((
            OdopErrorCode::Unsupported,
            format!("service '{scheme}' does not support {op_name}"),
        ))
    }
}

/// Convenience: fetch the operator's full capability once. Cheap (cached by
/// OpenDAL); the whole `Capability` is `Copy`.
pub(crate) fn full(op: &OdopOperator) -> Capability {
    op.op.info().full_capability()
}

// ── introspection FFI: capabilities as an index-addressable (name, bool) list ─

/// One capability flag: `name` (borrowed, NUL-terminated) + `supported`.
#[repr(C)]
pub struct OdopCapability {
    /// Capability name (e.g. "write_can_append"). Borrowed from the list; do
    /// NOT free. Valid until `odop_capabilities_free`.
    pub name: *const c_char,
    /// 1 if supported, 0 otherwise.
    pub supported: u8,
}

/// Opaque, index-addressable list of an operator's boolean capabilities.
pub struct OdopCapabilityList {
    items: Vec<(CString, bool)>,
}

/// Materialize every boolean capability of `op` into an index-addressable list.
///
/// Returns null only on null input or panic. Free with
/// `odop_capabilities_free`.
///
/// # Safety
/// `op` must be a live handle from `odop_operator_new`. `err` must be valid.
#[no_mangle]
pub unsafe extern "C" fn odop_capabilities(
    op: *const OdopOperator,
    err: *mut OdopError,
) -> *mut OdopCapabilityList {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if op.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null operator");
            return std::ptr::null_mut();
        }
        let cap = full(&*op);
        let items: Vec<(CString, bool)> = capability_bools(&cap)
            .iter()
            .map(|(name, sup)| (CString::new(*name).unwrap_or_default(), *sup))
            .collect();
        crate::error::set_ok(err);
        Box::into_raw(Box::new(OdopCapabilityList { items }))
    }));
    result.unwrap_or_else(|_| {
        set_error(err, OdopErrorCode::Panic, "panic in odop_capabilities");
        std::ptr::null_mut()
    })
}

/// Number of capability entries in the list. 0 on null.
///
/// # Safety
/// `list` must be null or a handle from `odop_capabilities`.
#[no_mangle]
pub unsafe extern "C" fn odop_capabilities_len(list: *const OdopCapabilityList) -> usize {
    if list.is_null() {
        return 0;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (*list).items.len())).unwrap_or(0)
}

/// Read capability `index` into `out`. Returns 1 on success, 0 on out-of-range
/// or null. `out.name` borrows from the list (valid until it is freed).
///
/// # Safety
/// `list` must be a live handle from `odop_capabilities`; `out` must be valid.
#[no_mangle]
pub unsafe extern "C" fn odop_capabilities_entry(
    list: *const OdopCapabilityList,
    index: usize,
    out: *mut OdopCapability,
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
/// `list` must be null or a handle from `odop_capabilities`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn odop_capabilities_free(list: *mut OdopCapabilityList) {
    if list.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(Box::from_raw(list));
    }));
}
