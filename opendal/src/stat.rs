//! `stat` across the FFI boundary.
//!
//! Returns a `#[repr(C)]` metadata struct by out-param. DuckDB uses this for
//! `GetFileSize` / `GetLastModifiedTime` / `GetFileType`.

use std::ffi::c_char;

use crate::capability::require;
use crate::error::{set_error, set_ok, set_opendal_error, OdError, OdErrorCode};
use crate::ffi::{cstr, ffi_guard};
use crate::operator::OdOperator;
use crate::runtime::block_on;

/// C-visible metadata for a path.
#[repr(C)]
pub struct OdMetadata {
    /// Content length in bytes.
    pub content_length: u64,
    /// Last-modified time in Unix milliseconds, or -1 if unknown.
    pub last_modified_ms: i64,
    /// 1 if this path is a directory, 0 otherwise.
    pub is_dir: u8,
}

impl OdMetadata {
    fn empty() -> Self {
        OdMetadata {
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
/// - `op` must be a live handle from `od_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - `out_meta` and `err` must be valid, writable pointers.
#[no_mangle]
pub unsafe extern "C" fn od_stat(
    op: *const OdOperator,
    path: *const c_char,
    out_meta: *mut OdMetadata,
    err: *mut OdError,
) {
    ffi_guard!(err, (), "od_stat", {
        if !out_meta.is_null() {
            *out_meta = OdMetadata::empty();
        }
        if op.is_null() || path.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null operator or path");
            return;
        }
        let odop = &*op;
        if let Err((code, msg)) = require(&odop.scheme, odop.cap.stat, "stat") {
            set_error(err, code, msg);
            return;
        }
        let path = match cstr(path) {
            Some(s) => s,
            None => {
                set_error(err, OdErrorCode::InvalidInput, "path is null or not UTF-8");
                return;
            }
        };

        match block_on(odop.op.stat(path)) {
            Ok(meta) => {
                if !out_meta.is_null() {
                    let last_modified_ms = meta
                        .last_modified()
                        .map(|t| t.into_inner().as_millisecond())
                        .unwrap_or(-1);
                    *out_meta = OdMetadata {
                        content_length: meta.content_length(),
                        last_modified_ms,
                        is_dir: if meta.is_dir() { 1 } else { 0 },
                    };
                }
                set_ok(err);
            }
            Err(e) => set_opendal_error(err, &e),
        }
    })
}

/// Check whether `path` exists. Returns 1 if it exists, 0 if not, -1 on error
/// (with `*err` populated).
///
/// # Safety
/// - `op` must be a live handle from `od_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - `err` must be a valid, writable pointer.
#[no_mangle]
pub unsafe extern "C" fn od_exists(
    op: *const OdOperator,
    path: *const c_char,
    err: *mut OdError,
) -> i8 {
    ffi_guard!(err, -1, "od_exists", {
        if op.is_null() || path.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null operator or path");
            return -1;
        }
        let odop = &*op;
        if let Err((code, msg)) = require(&odop.scheme, odop.cap.stat, "stat") {
            set_error(err, code, msg);
            return -1;
        }
        let path = match cstr(path) {
            Some(s) => s,
            None => {
                set_error(err, OdErrorCode::InvalidInput, "path is null or not UTF-8");
                return -1;
            }
        };
        match block_on(odop.op.exists(path)) {
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
    })
}
