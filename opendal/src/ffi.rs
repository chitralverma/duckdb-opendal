//! Shared FFI plumbing: panic guarding, handle freeing, and C-string decoding.
//!
//! Every `extern "C"` entry must catch panics (unwinding across the C ABI is
//! UB) and convert them into an `OdError`. Rather than repeat that boilerplate
//! in every function, entries wrap their body in [`ffi_guard!`], and opaque
//! handles are released through [`free_handle`].

use std::ffi::{c_char, CStr};

use crate::error::{set_error, OdError, OdErrorCode};

/// Decode a `*const c_char` into `&str`, or `None` on null / invalid UTF-8.
///
/// # Safety
/// `p` must be null or point to a valid NUL-terminated C string that outlives
/// the returned reference.
pub(crate) unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        None
    } else {
        CStr::from_ptr(p).to_str().ok()
    }
}

/// Free a `Box`-allocated opaque handle. Null is a no-op. Panic-safe.
///
/// # Safety
/// `p` must be null or a pointer from `Box::into_raw`, not already freed, with
/// no borrows still outstanding.
pub(crate) unsafe fn free_handle<T>(p: *mut T) {
    if p.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(Box::from_raw(p));
    }));
}

/// Run an FFI entry body under `catch_unwind`, converting a panic into an
/// `OdError` (code `Panic`) written to `$err` and returning `$fallback`.
///
/// Usage:
/// ```ignore
/// ffi_guard!(err, std::ptr::null_mut(), "od_reader_open", {
///     // body returning *mut OdReader
/// })
/// ```
macro_rules! ffi_guard {
    ($err:expr, $fallback:expr, $ctx:literal, $body:block) => {{
        let result = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| $body));
        match result {
            Ok(v) => v,
            Err(_) => {
                $crate::ffi::set_panic($err, concat!("panic in ", $ctx));
                $fallback
            }
        }
    }};
}

pub(crate) use ffi_guard;

/// Write a panic error into the out-param (helper for the `ffi_guard!` macro).
pub(crate) unsafe fn set_panic(err: *mut OdError, msg: &str) {
    set_error(err, OdErrorCode::Panic, msg);
}
