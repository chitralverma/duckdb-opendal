//! Filesystem mutations across the FFI boundary: create_dir, remove, rename.

use std::ffi::{c_char, CStr};
use std::future::IntoFuture;

use crate::error::{set_error, set_ok, set_opendal_error, OdopError, OdopErrorCode};
use crate::operator::OdopOperator;
use crate::runtime::block_on;

/// Read a `*const c_char` into `&str`, or None on null/invalid UTF-8.
unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

/// Create a directory at `path` (recursive, like `mkdir -p`). `path` should end
/// with '/'. Returns 0 on success, -1 on error.
///
/// # Safety
/// - `op` must be a live handle from `odop_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn odop_create_dir(
    op: *const OdopOperator,
    path: *const c_char,
    err: *mut OdopError,
) -> i32 {
    run(op, err, |o| {
        let p = match cstr(path) {
            Some(s) => s,
            None => return Err(MutErr::Invalid("path is null or not UTF-8".into())),
        };
        // OpenDAL requires a trailing slash to denote a directory.
        let p = if p.ends_with('/') {
            p.to_string()
        } else {
            format!("{p}/")
        };
        block_on(o.create_dir(&p)).map_err(MutErr::Opendal)
    })
}

/// Remove a file or (recursively) a directory at `path`. Returns 0 on success,
/// -1 on error.
///
/// # Safety
/// - `op` must be a live handle from `odop_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn odop_remove(
    op: *const OdopOperator,
    path: *const c_char,
    recursive: u8,
    err: *mut OdopError,
) -> i32 {
    run(op, err, |o| {
        let p = match cstr(path) {
            Some(s) => s,
            None => return Err(MutErr::Invalid("path is null or not UTF-8".into())),
        };
        if recursive != 0 {
            block_on(o.delete_with(p).recursive(true).into_future()).map_err(MutErr::Opendal)
        } else {
            block_on(o.delete(p)).map_err(MutErr::Opendal)
        }
    })
}

/// Rename/move `from` to `to` within the same operator. Returns 0 on success,
/// -1 on error. Not all services support server-side rename.
///
/// # Safety
/// - `op` must be a live handle from `odop_operator_new`.
/// - `from`/`to` must be valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn odop_rename(
    op: *const OdopOperator,
    from: *const c_char,
    to: *const c_char,
    err: *mut OdopError,
) -> i32 {
    run(op, err, |o| {
        let f = match cstr(from) {
            Some(s) => s,
            None => return Err(MutErr::Invalid("from is null or not UTF-8".into())),
        };
        let t = match cstr(to) {
            Some(s) => s,
            None => return Err(MutErr::Invalid("to is null or not UTF-8".into())),
        };
        block_on(o.rename(f, t)).map_err(MutErr::Opendal)
    })
}

// ── shared plumbing ──────────────────────────────────────────────────────────

/// A mutation error: either an OpenDAL error or an input-validation message.
enum MutErr {
    Opendal(opendal::Error),
    Invalid(String),
}

impl From<opendal::Error> for MutErr {
    fn from(e: opendal::Error) -> Self {
        MutErr::Opendal(e)
    }
}

/// Run a fallible operation against the operator, translating the result into
/// the FFI return code + error out-param, with panic protection.
unsafe fn run<F>(op: *const OdopOperator, err: *mut OdopError, f: F) -> i32
where
    F: FnOnce(&opendal::Operator) -> Result<(), MutErr>,
{
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if op.is_null() {
            set_error(err, OdopErrorCode::InvalidInput, "null operator");
            return -1i32;
        }
        let o = &(*op).op;
        match f(o) {
            Ok(()) => {
                set_ok(err);
                0
            }
            Err(MutErr::Opendal(e)) => {
                set_opendal_error(err, &e);
                -1
            }
            Err(MutErr::Invalid(msg)) => {
                set_error(err, OdopErrorCode::InvalidInput, msg);
                -1
            }
        }
    }));
    result.unwrap_or_else(|_| {
        set_error(err, OdopErrorCode::Panic, "panic in mutation");
        -1
    })
}
