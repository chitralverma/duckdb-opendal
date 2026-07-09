//! opendalfs-core — FFI core for the duckdb-opendal filesystem extension.
//!
//! Phase 0: minimal surface proving the C++ (DuckDB shell) ↔ Rust (OpenDAL core)
//! link works end-to-end. Real Operator/Reader/Writer/Lister surface arrives in
//! later phases.
//!
//! FFI safety rules (see docs/duckdb-opendal-AGENTS.md):
//!   - every `extern "C"` entry wraps its body in `catch_unwind` (panic across
//!     the C ABI is UB);
//!   - every allocation that crosses the boundary has a matching `*_free`;
//!   - strings are handed out as owned C strings and freed via `odop_string_free`.

use std::ffi::{c_char, CString};
use std::panic::catch_unwind;

/// Version string of this FFI core (crate version).
const OPENDALFS_CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Return a heap-allocated C string describing the opendalfs-core + OpenDAL
/// versions. Caller owns the pointer and MUST free it with `odop_string_free`.
///
/// Returns null on allocation failure or panic.
///
/// # Safety
/// The returned pointer must be freed exactly once via `odop_string_free` and
/// not used afterwards.
#[no_mangle]
pub extern "C" fn odop_version() -> *mut c_char {
    catch_unwind(|| {
        // opendal does not expose a const version string; report the crate we pin.
        let s = format!("opendalfs-core {OPENDALFS_CORE_VERSION} (opendal 0.57)");
        match CString::new(s) {
            Ok(c) => c.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Free a C string previously returned by this library (e.g. `odop_version`).
///
/// Safe to call with null (no-op).
///
/// # Safety
/// `ptr` must be either null or a pointer previously returned by this library
/// and not already freed.
#[no_mangle]
pub unsafe extern "C" fn odop_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    // Reconstruct and drop. Wrapped in catch_unwind for boundary safety.
    let _ = catch_unwind(|| {
        drop(CString::from_raw(ptr));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn version_roundtrip() {
        let p = odop_version();
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
        assert!(s.contains("opendalfs-core"));
        unsafe { odop_string_free(p) };
    }
}
