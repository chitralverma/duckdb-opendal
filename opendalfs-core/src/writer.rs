//! Writer across the FFI boundary — streaming, append-only writes.
//!
//! A DuckDB write handle maps to an OpenDAL `Writer`. Writers are append-only
//! (no seek): each `odop_writer_write` appends a chunk; `odop_writer_close`
//! finalizes (flushing buffered/multipart state). DuckDB's Parquet/CSV writers
//! emit data sequentially, which matches this model.

use std::ffi::c_char;

use opendal::Writer;

use crate::capability::{full, require};
use crate::error::{set_error, set_ok, set_opendal_error, OdopError, OdopErrorCode};
use crate::ffi::{cstr, ffi_guard, free_handle};
use crate::operator::OdopOperator;
use crate::runtime::block_on;

/// Opaque handle wrapping an `opendal::Writer`.
pub struct OdopWriter {
    writer: Writer,
}

/// Open a writer for `path` on `op`. Any existing object at `path` is
/// overwritten on close.
///
/// # Safety
/// - `op` must be a live handle from `odop_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - The returned handle must be freed once with `odop_writer_free`.
#[no_mangle]
pub unsafe extern "C" fn odop_writer_open(
    op: *const OdopOperator,
    path: *const c_char,
    err: *mut OdopError,
) -> *mut OdopWriter {
    ffi_guard!(err, std::ptr::null_mut(), "odop_writer_open", {
        if op.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null operator");
            return std::ptr::null_mut();
        }
        let odop = &*op;
        if let Err((code, msg)) = require(&odop.scheme, full(odop).write, "write") {
            set_error(err, code, msg);
            return std::ptr::null_mut();
        }
        let path = match cstr(path) {
            Some(s) => s,
            None => {
                set_error(err, OdopErrorCode::InvalidInput, "path is null or not UTF-8");
                return std::ptr::null_mut();
            }
        };
        match block_on(odop.op.writer(path)) {
            Ok(writer) => {
                set_ok(err);
                Box::into_raw(Box::new(OdopWriter { writer }))
            }
            Err(e) => {
                set_opendal_error(err, &e);
                std::ptr::null_mut()
            }
        }
    })
}

/// Append `len` bytes from `data` to the writer. Returns 0 on success, -1 on
/// error.
///
/// # Safety
/// - `writer` must be a live handle from `odop_writer_open`.
/// - `data` must point to at least `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn odop_writer_write(
    writer: *mut OdopWriter,
    data: *const u8,
    len: u64,
    err: *mut OdopError,
) -> i32 {
    ffi_guard!(err, -1, "odop_writer_write", {
        if writer.is_null() || (data.is_null() && len != 0) {
            set_error(err, OdopErrorCode::InvalidInput, "null writer or data");
            return -1;
        }
        if len == 0 {
            set_ok(err);
            return 0;
        }
        let writer = &mut (*writer).writer;
        // Copy the caller's bytes into an owned buffer for the async write.
        let bytes = std::slice::from_raw_parts(data, len as usize).to_vec();
        match block_on(writer.write(bytes)) {
            Ok(()) => {
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

/// Finalize the writer, flushing all buffered data (and completing any
/// multipart upload). Returns 0 on success, -1 on error. The writer must still
/// be freed with `odop_writer_free` afterwards.
///
/// # Safety
/// `writer` must be a live handle from `odop_writer_open`.
#[no_mangle]
pub unsafe extern "C" fn odop_writer_close(writer: *mut OdopWriter, err: *mut OdopError) -> i32 {
    ffi_guard!(err, -1, "odop_writer_close", {
        if writer.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null writer");
            return -1;
        }
        match block_on((*writer).writer.close()) {
            Ok(_meta) => {
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

/// Abort the writer, discarding buffered data (and cancelling any multipart
/// upload). Returns 0 on success, -1 on error.
///
/// # Safety
/// `writer` must be a live handle from `odop_writer_open`.
#[no_mangle]
pub unsafe extern "C" fn odop_writer_abort(writer: *mut OdopWriter, err: *mut OdopError) -> i32 {
    ffi_guard!(err, -1, "odop_writer_abort", {
        if writer.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null writer");
            return -1;
        }
        match block_on((*writer).writer.abort()) {
            Ok(()) => {
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

/// Free a writer handle. Safe to call with null (no-op). Does NOT flush — call
/// `odop_writer_close` first to persist data.
///
/// # Safety
/// `writer` must be null or a handle from `odop_writer_open`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn odop_writer_free(writer: *mut OdopWriter) {
    free_handle(writer);
}
