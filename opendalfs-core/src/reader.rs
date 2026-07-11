//! Reader across the FFI boundary — the positioned/ranged read path.
//!
//! A DuckDB positioned read maps to a single `odop_reader_read(handle, offset,
//! len, buf)` → one OpenDAL ranged read into the caller's buffer. The offset is
//! passed per call, so reads are stateless and atomic by construction.

use std::ffi::c_char;

use std::future::IntoFuture;

use opendal::Reader;

use crate::capability::require;
use crate::error::{set_error, set_ok, set_opendal_error, OdopError, OdopErrorCode};
use crate::ffi::{cstr, ffi_guard, free_handle};
use crate::operator::OdopOperator;
use crate::runtime::block_on;

/// Opaque handle wrapping an `opendal::Reader`. Free with `odop_reader_free`.
pub struct OdopReader {
    reader: Reader,
}

/// Open a reader for `path` on `op`.
///
/// # Safety
/// - `op` must be a live handle from `odop_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - The returned handle must be freed once with `odop_reader_free`.
#[no_mangle]
pub unsafe extern "C" fn odop_reader_open(
    op: *const OdopOperator,
    path: *const c_char,
    err: *mut OdopError,
) -> *mut OdopReader {
    ffi_guard!(err, std::ptr::null_mut(), "odop_reader_open", {
        if op.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null operator");
            return std::ptr::null_mut();
        }
        let odop = &*op;
        if let Err((code, msg)) = require(&odop.scheme, odop.cap.read, "read") {
            set_error(err, code, msg);
            return std::ptr::null_mut();
        }
        let path = match cstr(path) {
            Some(s) => s,
            None => {
                set_error(
                    err,
                    OdopErrorCode::InvalidInput,
                    "path is null or not UTF-8",
                );
                return std::ptr::null_mut();
            }
        };
        // Apply I/O tuning (concurrent/chunk) when configured; 0 = leave the
        // OpenDAL per-service default.
        let mut b = odop.op.reader_with(path);
        if odop.io.read.concurrent > 0 {
            b = b.concurrent(odop.io.read.concurrent);
        }
        if odop.io.read.chunk > 0 {
            b = b.chunk(odop.io.read.chunk);
        }
        match block_on(b.into_future()) {
            Ok(reader) => {
                set_ok(err);
                Box::into_raw(Box::new(OdopReader { reader }))
            }
            Err(e) => {
                set_opendal_error(err, &e);
                std::ptr::null_mut()
            }
        }
    })
}

/// Read up to `len` bytes starting at `offset` into `buf`.
///
/// Returns the number of bytes read (may be less than `len` at EOF), or -1 on
/// error. Reads straight into the caller's buffer.
///
/// # Safety
/// - `reader` must be a live handle from `odop_reader_open`.
/// - `buf` must point to at least `len` writable bytes.
/// - `err` must be a valid, writable pointer.
#[no_mangle]
pub unsafe extern "C" fn odop_reader_read(
    reader: *mut OdopReader,
    offset: u64,
    len: u64,
    buf: *mut u8,
    err: *mut OdopError,
) -> i64 {
    ffi_guard!(err, -1, "odop_reader_read", {
        if reader.is_null() || buf.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null reader or buffer");
            return -1;
        }
        if len == 0 {
            set_ok(err);
            return 0;
        }
        let reader = &mut (*reader).reader;
        let end = offset.saturating_add(len);
        // Read directly into the caller's buffer. `read_into` streams each chunk
        // straight into the destination (one copy), avoiding the flatten
        // allocation + second copy that `read()` + `Buffer::to_bytes()` incurs
        // for a multi-chunk Buffer. The range is bounded to `len`, so the yielded
        // bytes fit; we still expose the slice as a fixed-capacity BufMut.
        let mut dst: &mut [u8] = std::slice::from_raw_parts_mut(buf, len as usize);
        match block_on(reader.read_into(&mut dst, offset..end)) {
            Ok(n) => {
                set_ok(err);
                n as i64
            }
            Err(e) => {
                set_opendal_error(err, &e);
                -1
            }
        }
    })
}

/// Free a reader handle. Safe to call with null (no-op).
///
/// # Safety
/// `reader` must be null or a handle from `odop_reader_open`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn odop_reader_free(reader: *mut OdopReader) {
    free_handle(reader);
}
