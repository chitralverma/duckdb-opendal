//! `stat` across the FFI boundary.
//!
//! Returns a `#[repr(C)]` metadata struct by out-param. DuckDB uses this for
//! `GetFileSize` / `GetLastModifiedTime` / `GetFileType`.

use std::ffi::c_char;
use std::future::IntoFuture;

use futures::StreamExt;

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
        if op.is_null() || path.is_null() || out_meta.is_null() {
            set_error(
                err,
                OdErrorCode::InvalidInput,
                "null operator, path, or metadata output",
            );
            return;
        }
        *out_meta = OdMetadata::empty();
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
                let last_modified_ms = meta
                    .last_modified()
                    .map(|t| t.into_inner().as_millisecond())
                    .unwrap_or(-1);
                *out_meta = OdMetadata {
                    content_length: meta.content_length(),
                    last_modified_ms,
                    is_dir: if meta.is_dir() { 1 } else { 0 },
                };
                set_ok(err);
            }
            Err(e) => set_opendal_error(err, &e),
        }
    })
}

/// Check whether `path` names a directory. Returns 1 if it does, 0 if not, -1 on
/// error (with `*err` populated).
///
/// Object stores (s3, gcs, …) have no real directories — a "directory" is just a
/// key prefix with objects under it, and `stat` on the bare prefix returns
/// NotFound. So this first tries `stat` (which resolves real directory markers,
/// including local-fs directories), and on NotFound falls back to listing the
/// prefix and checking for at least one child. This lets DuckDB's
/// `DirectoryExists`-gated flows (e.g. partitioned `COPY ... OVERWRITE`, which
/// clears existing files via RemoveFiles) work on prefix-only backends.
///
/// Workaround for apache/opendal#6761 (stat cannot differentiate dir vs file
/// without a trailing-slash hint → NotFound on a bare prefix). Simplify to a
/// single stat once that upstream feature lands.
///
/// # Safety
/// - `op` must be a live handle from `od_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - `err` must be a valid, writable pointer.
#[no_mangle]
pub unsafe extern "C" fn od_dir_exists(
    op: *const OdOperator,
    path: *const c_char,
    err: *mut OdError,
) -> i8 {
    ffi_guard!(err, -1, "od_dir_exists", {
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

        // 1) stat resolves real directory markers (local fs, and backends that
        //    keep dir objects).
        match block_on(odop.op.stat(path)) {
            Ok(meta) => {
                if meta.is_dir() {
                    set_ok(err);
                    return 1;
                }
                // A file exists at this exact path — not a directory.
                set_ok(err);
                return 0;
            }
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => {
                // fall through to the prefix probe
            }
            Err(e) => {
                set_opendal_error(err, &e);
                return -1;
            }
        }

        // 2) Prefix probe: on object stores a directory is a non-empty key
        //    prefix. List with a trailing slash and look for one child.
        if !odop.cap.list {
            // No stat hit and the backend cannot list → treat as absent.
            set_ok(err);
            return 0;
        }
        let mut prefix = path.to_string();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        match block_on(odop.op.lister_with(&prefix).into_future()) {
            Ok(mut lister) => match block_on(lister.next()) {
                Some(Ok(_)) => {
                    set_ok(err);
                    1
                }
                Some(Err(e)) => {
                    set_opendal_error(err, &e);
                    -1
                }
                None => {
                    set_ok(err);
                    0
                }
            },
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => {
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
