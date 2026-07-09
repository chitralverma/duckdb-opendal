//! `stat` across the FFI boundary.
//!
//! Returns a `#[repr(C)]` metadata struct by out-param. DuckDB uses this for
//! `GetFileSize` / `GetLastModifiedTime` / `GetFileType`.

use std::ffi::{c_char, CStr};

use crate::error::{set_error, set_ok, set_opendal_error, OdopError, OdopErrorCode};
use crate::operator::OdopOperator;
use crate::runtime::block_on;

/// C-visible metadata for a path.
#[repr(C)]
pub struct OdopMetadata {
    /// Content length in bytes.
    pub content_length: u64,
    /// Last-modified time in Unix milliseconds, or -1 if unknown.
    pub last_modified_ms: i64,
    /// 1 if this path is a directory, 0 otherwise.
    pub is_dir: u8,
}

impl OdopMetadata {
    fn empty() -> Self {
        OdopMetadata {
            content_length: 0,
            last_modified_ms: -1,
            is_dir: 0,
        }
    }
}

/// Stat `path` on `op`, writing metadata into `out_meta`.
///
/// On success sets `*err` to Ok and fills `*out_meta`. On failure populates
/// `*err` and leaves `*out_meta` zeroed.
///
/// # Safety
/// - `op` must be a live handle from `odop_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - `out_meta` and `err` must be valid, writable pointers.
#[no_mangle]
pub unsafe extern "C" fn odop_stat(
    op: *const OdopOperator,
    path: *const c_char,
    out_meta: *mut OdopMetadata,
    err: *mut OdopError,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !out_meta.is_null() {
            *out_meta = OdopMetadata::empty();
        }
        if op.is_null() || path.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null operator or path");
            return;
        }
        let op = &(*op).op;
        let path = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_error(err, OdopErrorCode::InvalidInput, "path not UTF-8");
                return;
            }
        };

        match block_on(op.stat(path)) {
            Ok(meta) => {
                if !out_meta.is_null() {
                    let last_modified_ms = meta
                        .last_modified()
                        .map(|t| t.into_inner().as_millisecond())
                        .unwrap_or(-1);
                    *out_meta = OdopMetadata {
                        content_length: meta.content_length(),
                        last_modified_ms,
                        is_dir: if meta.is_dir() { 1 } else { 0 },
                    };
                }
                set_ok(err);
            }
            Err(e) => set_opendal_error(err, &e),
        }
    }));

    if result.is_err() {
        set_error(err, OdopErrorCode::Panic, "panic in odop_stat");
    }
}

/// Check whether `path` exists. Returns 1 if it exists, 0 if not, -1 on error
/// (with `*err` populated).
///
/// # Safety
/// - `op` must be a live handle from `odop_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - `err` must be a valid, writable pointer.
#[no_mangle]
pub unsafe extern "C" fn odop_exists(
    op: *const OdopOperator,
    path: *const c_char,
    err: *mut OdopError,
) -> i8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if op.is_null() || path.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null operator or path");
            return -1i8;
        }
        let op = &(*op).op;
        let path = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_error(err, OdopErrorCode::InvalidInput, "path not UTF-8");
                return -1;
            }
        };
        match block_on(op.exists(path)) {
            Ok(true) => {
                set_ok(err);
                1
            }
            Ok(false) => {
                set_ok(err);
                0
            }
            Err(e) => {
                set_opendal_error(err, &e);
                -1
            }
        }
    }));
    result.unwrap_or(-1)
}
