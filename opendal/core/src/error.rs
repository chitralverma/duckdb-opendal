//! FFI error type shared across the boundary.
//!
//! Every fallible FFI call reports failure through an out-param `OdError`
//! (a `#[repr(C)]` struct) rather than Rust's `Result`. `code == 0` means
//! success; non-zero maps to an `OdErrorCode`. The `message` is an owned C
//! string that the caller must free with `od_string_free` when non-null.

use std::ffi::{c_char, CString};

/// Error categories surfaced to the C++ side. Mirrors the subset of
/// `opendal::ErrorKind` we care about; everything else collapses to `Other`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OdErrorCode {
    Ok = 0,
    NotFound = 1,
    PermissionDenied = 2,
    NotADirectory = 3,
    IsADirectory = 4,
    AlreadyExists = 5,
    InvalidInput = 6,
    Unsupported = 7,
    Unexpected = 8,
    /// A Rust panic was caught at the FFI boundary.
    Panic = 98,
    /// Anything not otherwise categorized.
    Other = 99,
}

/// C-visible error out-param. Zero-initialized means success.
///
/// `message` is either null or an owned C string; free it with
/// `od_string_free`.
#[repr(C)]
pub struct OdError {
    pub code: OdErrorCode,
    pub message: *mut c_char,
}

impl OdError {
    /// A success value (no error).
    pub fn ok() -> Self {
        OdError {
            code: OdErrorCode::Ok,
            message: std::ptr::null_mut(),
        }
    }
}

/// Write an error into a caller-provided out-param pointer, if non-null.
/// Any prior `message` in the slot is NOT freed (callers pass a fresh slot).
pub(crate) unsafe fn set_error(out: *mut OdError, code: OdErrorCode, msg: impl Into<String>) {
    if out.is_null() {
        return;
    }
    let message = CString::new(msg.into())
        .unwrap_or_else(|_| CString::new("<error message contained NUL>").unwrap())
        .into_raw();
    *out = OdError { code, message };
}

/// Write a success value into the out-param, if non-null.
pub(crate) unsafe fn set_ok(out: *mut OdError) {
    if out.is_null() {
        return;
    }
    *out = OdError::ok();
}

/// Map an `opendal::Error` to our error code + message and store it.
pub(crate) unsafe fn set_opendal_error(out: *mut OdError, err: &opendal::Error) {
    use opendal::ErrorKind;
    let code = match err.kind() {
        ErrorKind::NotFound => OdErrorCode::NotFound,
        ErrorKind::PermissionDenied => OdErrorCode::PermissionDenied,
        ErrorKind::NotADirectory => OdErrorCode::NotADirectory,
        ErrorKind::IsADirectory => OdErrorCode::IsADirectory,
        ErrorKind::AlreadyExists => OdErrorCode::AlreadyExists,
        ErrorKind::ConfigInvalid | ErrorKind::Unsupported => OdErrorCode::Unsupported,
        _ => OdErrorCode::Unexpected,
    };
    set_error(out, code, err.to_string());
}
