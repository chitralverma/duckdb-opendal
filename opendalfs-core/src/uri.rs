//! URI parsing across the FFI boundary.
//!
//! Both the Rust core and the C++ shell parse `scheme://authority/path` URIs. To
//! avoid two divergent hand-rolled parsers, we delegate to OpenDAL's canonical
//! `OperatorUri` (the same type `from_uri` uses to build operators) and expose a
//! single `odop_parse_uri` FFI so the C++ side reuses it verbatim.
//!
//! `OperatorUri` splits a URI into scheme / authority / root (path with the
//! surrounding slashes trimmed) / options. Object-store schemes carry the
//! bucket/container in the authority; path-style schemes (fs) leave it empty
//! when the URI has a leading-slash path (`fs:///tmp/x`).

use std::ffi::{c_char, CStr, CString};

use opendal::OperatorUri;

use crate::error::{set_error, set_ok, OdopError, OdopErrorCode};

/// Parsed URI components, owned by an opaque handle (the strings must outlive
/// the borrowed pointers handed to C++). Free with `odop_parsed_uri_free`.
pub struct OdopParsedUri {
    scheme: CString,
    authority: CString,
    root: CString,
    has_authority: bool,
    has_root: bool,
}

/// C-visible view of a parsed URI. `scheme`/`authority`/`root` are borrowed,
/// NUL-terminated pointers into the `OdopParsedUri` handle (valid until it is
/// freed). `authority`/`root` point to empty strings when absent; the
/// `has_authority`/`has_root` flags disambiguate "absent" from "empty".
#[repr(C)]
pub struct OdopUriParts {
    pub scheme: *const c_char,
    pub authority: *const c_char,
    pub root: *const c_char,
    pub has_authority: u8,
    pub has_root: u8,
}

/// Parse `uri` with OpenDAL's canonical `OperatorUri`. On success returns a
/// non-null handle and fills `*out`; on failure returns null and sets `*err`.
///
/// # Safety
/// - `uri` must be a valid NUL-terminated C string.
/// - `out` and `err` must be valid, writable pointers.
/// - The returned handle must be freed once with `odop_parsed_uri_free`; the
///   pointers in `*out` are valid only until then.
#[no_mangle]
pub unsafe extern "C" fn odop_parse_uri(
    uri: *const c_char,
    out: *mut OdopUriParts,
    err: *mut OdopError,
) -> *mut OdopParsedUri {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let uri = match (!uri.is_null()).then(|| CStr::from_ptr(uri).to_str()) {
            Some(Ok(s)) => s,
            _ => {
                set_error(err, OdopErrorCode::InvalidInput, "uri is null or not UTF-8");
                return std::ptr::null_mut();
            }
        };

        let parsed = match OperatorUri::new(uri, Vec::<(String, String)>::new()) {
            Ok(p) => p,
            Err(e) => {
                crate::error::set_opendal_error(err, &e);
                return std::ptr::null_mut();
            }
        };

        let scheme = CString::new(parsed.scheme()).unwrap_or_default();
        let authority = CString::new(parsed.authority().unwrap_or("")).unwrap_or_default();
        let root = CString::new(parsed.root().unwrap_or("")).unwrap_or_default();
        let handle = Box::new(OdopParsedUri {
            has_authority: parsed.authority().is_some(),
            has_root: parsed.root().is_some(),
            scheme,
            authority,
            root,
        });

        if !out.is_null() {
            *out = OdopUriParts {
                scheme: handle.scheme.as_ptr(),
                authority: handle.authority.as_ptr(),
                root: handle.root.as_ptr(),
                has_authority: handle.has_authority as u8,
                has_root: handle.has_root as u8,
            };
        }
        set_ok(err);
        Box::into_raw(handle)
    }));
    result.unwrap_or_else(|_| {
        set_error(err, OdopErrorCode::Panic, "panic in odop_parse_uri");
        std::ptr::null_mut()
    })
}

/// Free a parsed-URI handle. Safe with null (no-op).
///
/// # Safety
/// `p` must be null or a handle from `odop_parse_uri`, not already freed, with
/// no borrowed pointers still in use.
#[no_mangle]
pub unsafe extern "C" fn odop_parsed_uri_free(p: *mut OdopParsedUri) {
    if p.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(Box::from_raw(p));
    }));
}

/// Internal helper: extract just the scheme from a URI via `OperatorUri`.
/// Returns "unknown" if parsing fails (used only for capability error messages).
pub(crate) fn scheme_of(uri: &str) -> String {
    OperatorUri::new(uri, Vec::<(String, String)>::new())
        .map(|p| p.scheme().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}
