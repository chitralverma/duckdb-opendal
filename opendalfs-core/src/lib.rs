//! opendalfs-core — FFI core for the duckdb-opendal filesystem extension.
//!
//! A C++ DuckDB `FileSystem` shell calls this Rust core (wrapping the OpenDAL
//! async crate) across a raw `extern "C"` boundary. A shared multi-thread Tokio
//! runtime bridges OpenDAL's async API to DuckDB's synchronous FS API.
//!
//! FFI safety rules (see docs/duckdb-opendal-AGENTS.md):
//!   - every `extern "C"` entry wraps its body in `catch_unwind` (panic across
//!     the C ABI is UB);
//!   - every allocation that crosses the boundary has a matching `*_free`;
//!   - errors are reported via the out-param `OdopError` (see `error.rs`);
//!   - strings handed out are owned C strings, freed via `odop_string_free`.

mod error;
mod layers;
mod lister;
mod operator;
mod reader;
mod runtime;
mod stat;

// Re-export the FFI surface so cbindgen picks it up from the crate root.
pub use error::{OdopError, OdopErrorCode};
pub use lister::{odop_list, odop_list_entry, odop_list_free, odop_list_len, OdopEntry, OdopEntryList};
pub use operator::{odop_operator_free, odop_operator_new, OdopOperator};
pub use reader::{odop_reader_free, odop_reader_open, odop_reader_read, OdopReader};
pub use stat::{odop_exists, odop_stat, OdopMetadata};

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
        let s = format!("opendalfs-core {OPENDALFS_CORE_VERSION} (opendal 0.57)");
        match CString::new(s) {
            Ok(c) => c.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Free a C string previously returned by this library (e.g. `odop_version`,
/// or an `OdopError::message`).
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
    let _ = catch_unwind(|| {
        drop(CString::from_raw(ptr));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    #[test]
    fn version_roundtrip() {
        let p = odop_version();
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
        assert!(s.contains("opendalfs-core"));
        unsafe { odop_string_free(p) };
    }

    #[test]
    fn memory_operator_write_stat_read() {
        // Build a memory operator, write via the async API on the runtime, then
        // exercise the FFI stat + reader paths end to end.
        let scheme = CString::new("memory").unwrap();
        let mut err = OdopError::ok();
        let op = unsafe {
            odop_operator_new(scheme.as_ptr(), std::ptr::null(), std::ptr::null(), 0, std::ptr::null(), std::ptr::null(), 0, &mut err)
        };
        assert!(!op.is_null(), "operator_new failed: code {}", err.code as i32);

        // Seed a value using the underlying operator directly.
        let payload = b"hello opendal fs".to_vec();
        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("greeting.txt", payload.clone())).unwrap();
        }

        // stat
        let path = CString::new("greeting.txt").unwrap();
        let mut meta = OdopMetadata {
            content_length: 0,
            last_modified_ms: 0,
            is_dir: 9,
        };
        let mut serr = OdopError::ok();
        unsafe { odop_stat(op, path.as_ptr(), &mut meta, &mut serr) };
        assert_eq!(serr.code as i32, OdopErrorCode::Ok as i32);
        assert_eq!(meta.content_length, payload.len() as u64);
        assert_eq!(meta.is_dir, 0);

        // reader: positioned read of the middle slice
        let mut rerr = OdopError::ok();
        let reader = unsafe { odop_reader_open(op, path.as_ptr(), &mut rerr) };
        assert!(!reader.is_null());
        let mut buf = vec![0u8; 6];
        let n = unsafe { odop_reader_read(reader, 6, 6, buf.as_mut_ptr(), &mut rerr) };
        assert_eq!(n, 6);
        assert_eq!(&buf, b"openda");

        unsafe { odop_reader_free(reader) };
        unsafe { odop_operator_free(op) };
    }

    #[test]
    fn memory_list_and_exists() {
        let scheme = CString::new("memory").unwrap();
        let mut err = OdopError::ok();
        let op = unsafe {
            odop_operator_new(scheme.as_ptr(), std::ptr::null(), std::ptr::null(), 0, std::ptr::null(), std::ptr::null(), 0, &mut err)
        };
        assert!(!op.is_null());

        // Seed a couple files under a/ .
        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("a/one.txt", b"11".to_vec())).unwrap();
            crate::runtime::block_on(inner.write("a/two.txt", b"222".to_vec())).unwrap();
        }

        // exists
        let p_one = CString::new("a/one.txt").unwrap();
        let p_missing = CString::new("a/nope.txt").unwrap();
        let mut e = OdopError::ok();
        assert_eq!(unsafe { odop_exists(op, p_one.as_ptr(), &mut e) }, 1);
        assert_eq!(unsafe { odop_exists(op, p_missing.as_ptr(), &mut e) }, 0);

        // list a/ recursively
        let dir = CString::new("a/").unwrap();
        let mut lerr = OdopError::ok();
        let list = unsafe { odop_list(op, dir.as_ptr(), 1, &mut lerr) };
        assert!(!list.is_null());
        let n = unsafe { odop_list_len(list) };
        // Expect our two files (dir markers may or may not appear depending on backend).
        let mut files = 0;
        for i in 0..n {
            let mut ent = OdopEntry {
                path: std::ptr::null(),
                name: std::ptr::null(),
                content_length: 0,
                last_modified_ms: 0,
                is_dir: 0,
            };
            assert_eq!(unsafe { odop_list_entry(list, i, &mut ent) }, 1);
            if ent.is_dir == 0 {
                files += 1;
                let name = unsafe { CStr::from_ptr(ent.name) }.to_str().unwrap();
                assert!(name == "one.txt" || name == "two.txt");
            }
        }
        assert_eq!(files, 2);

        unsafe { odop_list_free(list) };
        unsafe { odop_operator_free(op) };
    }

    #[test]
    fn memory_operator_with_layers() {
        // Build a memory operator with retry + timeout + concurrent-limit layers
        // and confirm it still reads/writes (layers are transparent to callers).
        let scheme = CString::new("memory").unwrap();
        let lk: Vec<CString> = ["retry.max_times", "timeout.seconds", "concurrent_limit"]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
        let lv: Vec<CString> = ["3", "30", "8"].iter().map(|s| CString::new(*s).unwrap()).collect();
        let lk_ptrs: Vec<*const c_char> = lk.iter().map(|c| c.as_ptr()).collect();
        let lv_ptrs: Vec<*const c_char> = lv.iter().map(|c| c.as_ptr()).collect();

        let mut err = OdopError::ok();
        let op = unsafe {
            odop_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                lk_ptrs.as_ptr(),
                lv_ptrs.as_ptr(),
                lk_ptrs.len(),
                &mut err,
            )
        };
        assert!(!op.is_null(), "layered operator_new failed: code {}", err.code as i32);

        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("layered.txt", b"ok".to_vec())).unwrap();
        }
        let path = CString::new("layered.txt").unwrap();
        let mut meta = OdopMetadata {
            content_length: 0,
            last_modified_ms: 0,
            is_dir: 9,
        };
        let mut serr = OdopError::ok();
        unsafe { odop_stat(op, path.as_ptr(), &mut meta, &mut serr) };
        assert_eq!(serr.code as i32, OdopErrorCode::Ok as i32);
        assert_eq!(meta.content_length, 2);

        unsafe { odop_operator_free(op) };
    }
}



