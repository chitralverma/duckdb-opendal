//! Reader across the FFI boundary — the positioned/ranged read path.
//!
//! A DuckDB positioned read maps to a single `odop_reader_read(handle, offset,
//! len, buf)` call → one OpenDAL ranged read into the caller's buffer. There is
//! no separate seek step at the boundary (see plan §5.2): the offset is passed
//! per call, so reads are stateless and atomic by construction.

use std::ffi::{c_char, CStr};

use opendal::Reader;

use crate::error::{set_error, set_ok, set_opendal_error, OdopError, OdopErrorCode};
use crate::operator::OdopOperator;
use crate::runtime::block_on;

/// Opaque handle wrapping an `opendal::Reader`.
///
/// The reader borrows nothing from the operator directly (OpenDAL readers are
/// self-contained once created), but conceptually must not outlive the process
/// runtime. Free with `odop_reader_free`.
pub struct OdopReader {
    reader: Reader,
}

/// Open a reader for `path` on `op`.
///
/// On success returns a non-null `*mut OdopReader` and sets `*err` to Ok.
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
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if op.is_null() || path.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null operator or path");
            return std::ptr::null_mut();
        }
        let op = &(*op).op;
        let path = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_error(err, OdopErrorCode::InvalidInput, "path not UTF-8");
                return std::ptr::null_mut();
            }
        };

        match block_on(op.reader(path)) {
            Ok(reader) => {
                set_ok(err);
                Box::into_raw(Box::new(OdopReader { reader }))
            }
            Err(e) => {
                set_opendal_error(err, &e);
                std::ptr::null_mut()
            }
        }
    }));

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            set_error(err, OdopErrorCode::Panic, "panic in odop_reader_open");
            std::ptr::null_mut()
        }
    }
}

/// Read up to `len` bytes starting at `offset` into `buf`.
///
/// Returns the number of bytes actually read (may be less than `len` at EOF),
/// or -1 on error (with `*err` populated). Reads straight into the caller's
/// buffer — no intermediate copy beyond OpenDAL's own buffer.
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
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if reader.is_null() || buf.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null reader or buffer");
            return -1i64;
        }
        if len == 0 {
            set_ok(err);
            return 0;
        }
        let reader = &mut (*reader).reader;
        let end = offset.saturating_add(len);

        match block_on(reader.read(offset..end)) {
            Ok(buffer) => {
                // `buffer` is an opendal::Buffer (possibly non-contiguous). Copy
                // its bytes into the caller's buffer, clamped to `len`.
                let to_copy = std::cmp::min(buffer.len() as u64, len) as usize;
                let dst = std::slice::from_raw_parts_mut(buf, to_copy);
                let bytes = buffer.to_bytes();
                dst.copy_from_slice(&bytes[..to_copy]);
                set_ok(err);
                to_copy as i64
            }
            Err(e) => {
                set_opendal_error(err, &e);
                -1
            }
        }
    }));

    match result {
        Ok(n) => n,
        Err(_) => {
            set_error(err, OdopErrorCode::Panic, "panic in odop_reader_read");
            -1
        }
    }
}

/// Free a reader handle. Safe to call with null (no-op).
///
/// # Safety
/// `reader` must be null or a handle from `odop_reader_open`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn odop_reader_free(reader: *mut OdopReader) {
    if reader.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(Box::from_raw(reader));
    }));
}
