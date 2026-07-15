//! duckdb-opendal — FFI core for the duckdb-opendal filesystem extension.
//!
//! A C++ DuckDB `FileSystem` shell calls this Rust core (wrapping the OpenDAL
//! async crate) across a raw `extern "C"` boundary. A shared multi-thread Tokio
//! runtime bridges OpenDAL's async API to DuckDB's synchronous FS API.
//!
//! FFI safety rules (see docs/duckdb-opendal-AGENTS.md):
//!   - every `extern "C"` entry wraps its body in `catch_unwind` (panic across
//!     the C ABI is UB);
//!   - every allocation that crosses the boundary has a matching `*_free`;
//!   - errors are reported via the out-param `OdError` (see `error.rs`);
//!   - strings handed out are owned C strings, freed via `od_string_free`.

mod capability;
mod config;
mod error;
mod ffi;
mod lister;
mod mutate;
mod operator;
mod reader;
mod runtime;
mod stat;
mod table;
mod uri;
mod writer;

// Re-export the FFI surface so cbindgen picks it up from the crate root.
pub use capability::{
    od_capabilities, od_capabilities_entry, od_capabilities_free, od_capabilities_len,
    od_operator_supports, OdCapability, OdCapabilityList,
};
pub use error::{OdError, OdErrorCode};
pub use lister::{od_list, od_list_entry, od_list_free, od_list_len, OdEntry, OdEntryList};
pub use mutate::{od_copy, od_create_dir, od_remove, od_rename};
pub use operator::{
    od_operator_free, od_operator_new, od_operator_warning, od_scheme_supported, od_schemes,
    od_schemes_entry, od_schemes_free, od_schemes_len, OdOperator, OdSchemeList,
};
pub use reader::{od_reader_free, od_reader_open, od_reader_read, OdReader};
pub use stat::{od_exists, od_stat, OdMetadata};
pub use table::{
    od_copy_cursor_free, od_copy_cursor_next, od_du_cursor_free, od_du_cursor_next,
    od_table_copy_open, od_table_cursor_free, od_table_cursor_next, od_table_du_open,
    od_table_glob_open, od_table_list_open, od_table_stat_open, OdCopyCursor, OdCopyOptions,
    OdCopyRow, OdDuCursor, OdDuRow, OdEntryMetadata, OdEntryRow, OdListOptions, OdStatOptions,
    OdTableCursor,
};
pub use writer::{
    od_writer_abort, od_writer_close, od_writer_free, od_writer_open, od_writer_write, OdWriter,
};

use std::ffi::{c_char, CString};
use std::panic::catch_unwind;

const URL_PATH_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'?');

/// Resolve a public URL into registered scheme, authority, and operation path.
///
/// # Safety
/// `url` must be a valid C string; `out`/`err` must be null or writable.
#[no_mangle]
pub unsafe extern "C" fn od_url_resolve(
    url: *const c_char,
    out_scheme: *mut *mut c_char,
    out_authority: *mut *mut c_char,
    out_path: *mut *mut c_char,
    err: *mut OdError,
) -> i32 {
    crate::ffi::ffi_guard!(err, -1, "od_url_resolve", {
        if out_scheme.is_null() || out_authority.is_null() || out_path.is_null() {
            crate::error::set_error(err, OdErrorCode::InvalidInput, "null resolved-url output");
            return -1;
        }
        *out_scheme = std::ptr::null_mut();
        *out_authority = std::ptr::null_mut();
        *out_path = std::ptr::null_mut();
        match crate::uri::resolve(url) {
            Ok((scheme, authority, path)) => {
                *out_scheme = scheme.into_raw();
                *out_authority = authority.into_raw();
                *out_path = path.into_raw();
                crate::error::set_ok(err);
                0
            }
            Err((code, message)) => {
                crate::error::set_error(err, code, message);
                -1
            }
        }
    })
}

/// Match an OpenDAL-relative path against the extension's full-path glob syntax.
///
/// # Safety
/// `pattern` and `path` must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn od_glob_match(pattern: *const c_char, path: *const c_char) -> u8 {
    catch_unwind(|| {
        let pattern = crate::ffi::cstr(pattern)?;
        let path = crate::ffi::cstr(path)?;
        Some(crate::table::glob_matches(pattern.as_bytes(), path.as_bytes()) as u8)
    })
    .ok()
    .flatten()
    .unwrap_or(0)
}

/// Build a public URL from resolved scheme/authority and operation path.
#[no_mangle]
pub unsafe extern "C" fn od_url_build(
    scheme: *const c_char,
    authority: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    catch_unwind(|| {
        let scheme = crate::ffi::cstr(scheme)?;
        let authority = crate::ffi::cstr(authority)?;
        let path = crate::ffi::cstr(path)?;
        let encoded = path
            .split('/')
            .map(|segment| {
                percent_encoding::utf8_percent_encode(segment, URL_PATH_ENCODE_SET).to_string()
            })
            .collect::<Vec<_>>()
            .join("/");
        CString::new(format!("{scheme}://{authority}/{encoded}"))
            .ok()
            .map(CString::into_raw)
    })
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}

/// Resolved OpenDAL crate version, injected by build.rs from this crate's
/// Cargo.toml dependency pin (opendal exposes no public VERSION const). Falls
/// back to "unknown" if unresolved.
const OPENDAL_VERSION: &str = env!("OPENDAL_VERSION");

/// One-time process initialization — call once at extension load, before any
/// operator is built.
///
/// Populates OpenDAL's service registry (we link opendal as a `staticlib`, so
/// its `#[ctor]` init can be dropped by the linker) and installs the rustls
/// `ring` crypto provider + reqwest HTTP transport (the `rustls-no-provider`
/// feature auto-installs neither). Idempotent (guarded by a `Once`).
///
/// # Safety
/// `err` must be null or a valid, writable pointer.
#[no_mangle]
pub unsafe extern "C" fn od_init(err: *mut OdError) -> i32 {
    use std::sync::OnceLock;
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    crate::ffi::ffi_guard!(err, -1, "od_init", {
        match INIT.get_or_init(|| {
            opendal::init_default_registry();
            // Another extension may already have installed a process provider.
            if rustls::crypto::CryptoProvider::get_default().is_none()
                && rustls::crypto::ring::default_provider()
                    .install_default()
                    .is_err()
                && rustls::crypto::CryptoProvider::get_default().is_none()
            {
                return Err("failed to install rustls crypto provider".to_string());
            }
            opendal::HttpTransporter::install_default(
                opendal_http_transport_reqwest::ReqwestTransport::default(),
            );
            Ok(())
        }) {
            Ok(()) => {
                crate::error::set_ok(err);
                0
            }
            Err(message) => {
                crate::error::set_error(err, OdErrorCode::Unexpected, message);
                -1
            }
        }
    })
}

/// Return the resolved OpenDAL library version (e.g. "0.58.0") as an owned C
/// string. Resolved from Cargo.toml at build time (opendal exposes no public
/// VERSION const). Caller MUST free it with `od_string_free`.
///
/// Returns null on allocation failure or panic. The C++ side composes the full
/// `opendal <ext-version> (opendal-core <this>)` version string.
///
/// # Safety
/// The returned pointer must be freed exactly once via `od_string_free`.
#[no_mangle]
pub extern "C" fn od_opendal_version() -> *mut c_char {
    catch_unwind(|| match CString::new(OPENDAL_VERSION) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Free a C string previously returned by this library (e.g. `od_opendal_version`,
/// or an `OdError::message`).
///
/// Safe to call with null (no-op).
///
/// # Safety
/// `ptr` must be either null or a pointer previously returned by this library
/// and not already freed.
#[no_mangle]
pub unsafe extern "C" fn od_string_free(ptr: *mut c_char) {
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

    fn init() {
        let mut err = OdError::ok();
        assert_eq!(unsafe { od_init(&mut err) }, 0);
        assert_eq!(err.code as i32, OdErrorCode::Ok as i32);
    }

    #[test]
    fn opendal_version_roundtrip() {
        let p = od_opendal_version();
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
        // Resolved from Cargo.toml at build time (not hardcoded/unknown).
        assert!(
            !s.is_empty() && s != "unknown",
            "opendal version unresolved: {s}"
        );
        // Looks like a semver (e.g. "0.58.0").
        assert!(
            s.chars().next().unwrap().is_ascii_digit(),
            "unexpected: {s}"
        );
        unsafe { od_string_free(p) };
    }

    #[test]
    fn memory_operator_write_stat_read() {
        // Build a memory operator, write via the async API on the runtime, then
        // exercise the FFI stat + reader paths end to end.
        let scheme = CString::new("memory").unwrap();
        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(
            !op.is_null(),
            "operator_new failed: code {}",
            err.code as i32
        );

        // Seed a value using the underlying operator directly.
        let payload = b"hello opendal fs".to_vec();
        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("greeting.txt", payload.clone())).unwrap();
        }

        // stat
        let path = CString::new("greeting.txt").unwrap();
        let mut meta = OdMetadata {
            content_length: 0,
            last_modified_ms: 0,
            is_dir: 9,
        };
        let mut serr = OdError::ok();
        unsafe { od_stat(op, path.as_ptr(), &mut meta, &mut serr) };
        assert_eq!(serr.code as i32, OdErrorCode::Ok as i32);
        assert_eq!(meta.content_length, payload.len() as u64);
        assert_eq!(meta.is_dir, 0);

        // reader: positioned read of the middle slice
        let mut rerr = OdError::ok();
        let reader = unsafe { od_reader_open(op, path.as_ptr(), &mut rerr) };
        assert!(!reader.is_null());
        let mut buf = vec![0u8; 6];
        let n = unsafe { od_reader_read(reader, 6, 6, buf.as_mut_ptr(), &mut rerr) };
        assert_eq!(n, 6);
        assert_eq!(&buf, b"openda");

        let n = unsafe { od_reader_read(reader, u64::MAX, 2, buf.as_mut_ptr(), &mut rerr) };
        assert_eq!(n, -1);
        assert_eq!(rerr.code as i32, OdErrorCode::InvalidInput as i32);
        let message = unsafe { CStr::from_ptr(rerr.message) }.to_str().unwrap();
        assert!(message.contains("range overflows"), "{message}");
        unsafe { od_string_free(rerr.message) };

        unsafe { od_reader_free(reader) };
        unsafe { od_operator_free(op) };
    }

    #[test]
    fn memory_list_and_exists() {
        let scheme = CString::new("memory").unwrap();
        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
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
        let mut e = OdError::ok();
        assert_eq!(unsafe { od_exists(op, p_one.as_ptr(), &mut e) }, 1);
        assert_eq!(unsafe { od_exists(op, p_missing.as_ptr(), &mut e) }, 0);

        // list a/ recursively
        let dir = CString::new("a/").unwrap();
        let mut lerr = OdError::ok();
        let list = unsafe { od_list(op, dir.as_ptr(), 1, &mut lerr) };
        assert!(!list.is_null());
        let n = unsafe { od_list_len(list) };
        // Expect our two files (dir markers may or may not appear depending on backend).
        let mut files = 0;
        for i in 0..n {
            let mut ent = OdEntry {
                path: std::ptr::null(),
                name: std::ptr::null(),
                content_length: 0,
                last_modified_ms: 0,
                is_dir: 0,
            };
            assert_eq!(unsafe { od_list_entry(list, i, &mut ent) }, 1);
            if ent.is_dir == 0 {
                files += 1;
                let name = unsafe { CStr::from_ptr(ent.name) }.to_str().unwrap();
                assert!(name == "one.txt" || name == "two.txt");
            }
        }
        assert_eq!(files, 2);

        unsafe { od_list_free(list) };
        unsafe { od_operator_free(op) };
    }

    #[test]
    fn memory_writer_and_mutations() {
        let scheme = CString::new("memory").unwrap();
        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(!op.is_null());

        // Write "hello world" in two chunks through the streaming writer.
        let path = CString::new("out/greeting.txt").unwrap();
        let mut werr = OdError::ok();
        let w = unsafe { od_writer_open(op, path.as_ptr(), &mut werr) };
        assert!(!w.is_null(), "writer_open failed: {}", werr.code as i32);
        let p1 = b"hello ";
        let p2 = b"world";
        assert_eq!(
            unsafe { od_writer_write(w, p1.as_ptr(), u64::MAX, &mut werr) },
            -1
        );
        assert_eq!(werr.code as i32, OdErrorCode::InvalidInput as i32);
        unsafe { od_string_free(werr.message) };
        werr = OdError::ok();
        assert_eq!(
            unsafe { od_writer_write(w, p1.as_ptr(), p1.len() as u64, &mut werr) },
            0
        );
        assert_eq!(
            unsafe { od_writer_write(w, p2.as_ptr(), p2.len() as u64, &mut werr) },
            0
        );
        assert_eq!(unsafe { od_writer_close(w, &mut werr) }, 0);
        assert_eq!(
            unsafe { od_writer_write(w, p2.as_ptr(), p2.len() as u64, &mut werr) },
            -1
        );
        assert_eq!(werr.code as i32, OdErrorCode::InvalidInput as i32);
        unsafe { od_string_free(werr.message) };
        werr = OdError::ok();
        assert_eq!(unsafe { od_writer_close(w, &mut werr) }, -1);
        assert_eq!(werr.code as i32, OdErrorCode::InvalidInput as i32);
        unsafe { od_string_free(werr.message) };
        werr = OdError::ok();
        assert_eq!(unsafe { od_writer_abort(w, &mut werr) }, -1);
        assert_eq!(werr.code as i32, OdErrorCode::InvalidInput as i32);
        unsafe { od_string_free(werr.message) };
        unsafe { od_writer_free(w) };

        // Read it back and verify content + size.
        let mut meta = OdMetadata {
            content_length: 0,
            last_modified_ms: 0,
            is_dir: 9,
        };
        let mut serr = OdError::ok();
        unsafe { od_stat(op, path.as_ptr(), &mut meta, &mut serr) };
        assert_eq!(serr.code as i32, OdErrorCode::Ok as i32);
        assert_eq!(meta.content_length, 11);

        let r = unsafe { od_reader_open(op, path.as_ptr(), &mut serr) };
        assert!(!r.is_null());
        assert_eq!(
            unsafe { od_reader_read(r, 0, 0, std::ptr::null_mut(), &mut serr) },
            0
        );
        let mut buf = vec![0u8; 11];
        let n = unsafe { od_reader_read(r, 0, 11, buf.as_mut_ptr(), &mut serr) };
        assert_eq!(n, 11);
        assert_eq!(&buf, b"hello world");
        unsafe { od_reader_free(r) };

        unsafe { od_stat(op, path.as_ptr(), std::ptr::null_mut(), &mut serr) };
        assert_eq!(serr.code as i32, OdErrorCode::InvalidInput as i32);
        unsafe { od_string_free(serr.message) };

        // rename → if the backend supports it, the old path is gone and the new
        // one exists. The memory service does not support server-side rename, so
        // tolerate Unsupported here (the C++ layer falls back to copy+delete).
        let dst = CString::new("out/renamed.txt").unwrap();
        let mut merr = OdError::ok();
        let rc = unsafe { od_rename(op, path.as_ptr(), dst.as_ptr(), &mut merr) };
        if rc == 0 {
            assert_eq!(unsafe { od_exists(op, path.as_ptr(), &mut merr) }, 0);
            assert_eq!(unsafe { od_exists(op, dst.as_ptr(), &mut merr) }, 1);
            // remove the renamed file.
            assert_eq!(unsafe { od_remove(op, dst.as_ptr(), 0, &mut merr) }, 0);
            assert_eq!(unsafe { od_exists(op, dst.as_ptr(), &mut merr) }, 0);
        } else {
            assert_eq!(merr.code as i32, OdErrorCode::Unsupported as i32);
            unsafe { od_string_free(merr.message) };
            // remove the original file instead.
            let mut rerr = OdError::ok();
            assert_eq!(unsafe { od_remove(op, path.as_ptr(), 0, &mut rerr) }, 0);
            assert_eq!(unsafe { od_exists(op, path.as_ptr(), &mut rerr) }, 0);
        }

        unsafe { od_operator_free(op) };
    }

    #[test]
    fn memory_operator_with_config_sections() {
        // Build a memory operator with retry + timeout + concurrent-limit layers
        // and confirm it still reads/writes (layers are transparent to callers).
        let scheme = CString::new("memory").unwrap();
        let lk: Vec<CString> = [
            "retry.max_times",
            "timeout.operation_timeout",
            "io.concurrent_limit",
        ]
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
        let lv: Vec<CString> = ["3", "30", "8"]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
        let lk_ptrs: Vec<*const c_char> = lk.iter().map(|c| c.as_ptr()).collect();
        let lv_ptrs: Vec<*const c_char> = lv.iter().map(|c| c.as_ptr()).collect();

        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                lk_ptrs.as_ptr(),
                lv_ptrs.as_ptr(),
                lk_ptrs.len(),
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(
            !op.is_null(),
            "layered operator_new failed: code {}",
            err.code as i32
        );

        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("layered.txt", b"ok".to_vec())).unwrap();
        }
        let path = CString::new("layered.txt").unwrap();
        let mut meta = OdMetadata {
            content_length: 0,
            last_modified_ms: 0,
            is_dir: 9,
        };
        let mut serr = OdError::ok();
        unsafe { od_stat(op, path.as_ptr(), &mut meta, &mut serr) };
        assert_eq!(serr.code as i32, OdErrorCode::Ok as i32);
        assert_eq!(meta.content_length, 2);

        unsafe { od_operator_free(op) };
    }

    #[test]
    fn memory_operator_with_cache() {
        let scheme = CString::new("memory").unwrap();
        // Section presence enables the cache; no implementation-specific
        // `enable` key is required.
        let lk: Vec<CString> = ["cache.memory_size"]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
        let lv: Vec<CString> = ["16"].iter().map(|s| CString::new(*s).unwrap()).collect();
        let lk_ptrs: Vec<*const c_char> = lk.iter().map(|c| c.as_ptr()).collect();
        let lv_ptrs: Vec<*const c_char> = lv.iter().map(|c| c.as_ptr()).collect();

        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                lk_ptrs.as_ptr(),
                lv_ptrs.as_ptr(),
                lk_ptrs.len(),
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(
            !op.is_null(),
            "foyer operator_new failed: code {}",
            err.code as i32
        );

        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("cached.txt", b"cache me".to_vec())).unwrap();
        }
        let path = CString::new("cached.txt").unwrap();
        // Read twice — second read should be served from the cache.
        for _ in 0..2 {
            let mut serr = OdError::ok();
            let r = unsafe { od_reader_open(op, path.as_ptr(), &mut serr) };
            assert!(!r.is_null());
            let mut buf = vec![0u8; 8];
            let n = unsafe { od_reader_read(r, 0, 8, buf.as_mut_ptr(), &mut serr) };
            assert_eq!(n, 8);
            assert_eq!(&buf, b"cache me");
            unsafe { od_reader_free(r) };
        }

        unsafe { od_operator_free(op) };
    }

    #[test]
    fn s3_operator_from_uri_extracts_bucket() {
        // from_uri must map the URI authority to the s3 `bucket` config via
        // OpenDAL's per-service parsing — no bucket key passed explicitly.
        // Uses a dummy endpoint/creds; no network I/O (operator build is lazy).
        init(); // register services (no auto-register ctor)
        let uri = CString::new("s3://my-bucket").unwrap();
        let keys: Vec<CString> = ["endpoint", "region", "access_key_id", "secret_access_key"]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
        let vals: Vec<CString> = ["http://127.0.0.1:1", "us-east-1", "x", "y"]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
        let k_ptrs: Vec<*const c_char> = keys.iter().map(|c| c.as_ptr()).collect();
        let v_ptrs: Vec<*const c_char> = vals.iter().map(|c| c.as_ptr()).collect();

        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                uri.as_ptr(),
                k_ptrs.as_ptr(),
                v_ptrs.as_ptr(),
                k_ptrs.len(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(
            !op.is_null(),
            "s3 from_uri operator_new failed: code {}",
            err.code as i32
        );
        // The operator's name should be the bucket parsed from the authority.
        let name = unsafe { (*op).op.info().name().to_string() };
        assert_eq!(name, "my-bucket");
        assert_eq!(unsafe { &(*op).scheme }, "s3");
        unsafe { od_operator_free(op) };
    }

    #[test]
    fn memory_operator_with_disk_cache() {
        let dir = tempfile::tempdir().unwrap();
        let disk_path = dir.path().to_str().unwrap().to_string();

        let scheme = CString::new("memory").unwrap();
        let keys = [
            "cache.memory_size",
            "cache.disk_path",
            "cache.disk_size",
            "cache.block_size",
        ];
        let vals = ["16 MiB", disk_path.as_str(), "64 MiB", "1 MiB"];
        let lk: Vec<CString> = keys.iter().map(|s| CString::new(*s).unwrap()).collect();
        let lv: Vec<CString> = vals.iter().map(|s| CString::new(*s).unwrap()).collect();
        let lk_ptrs: Vec<*const c_char> = lk.iter().map(|c| c.as_ptr()).collect();
        let lv_ptrs: Vec<*const c_char> = lv.iter().map(|c| c.as_ptr()).collect();

        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                lk_ptrs.as_ptr(),
                lv_ptrs.as_ptr(),
                lk_ptrs.len(),
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(
            !op.is_null(),
            "foyer-disk operator_new failed: code {}",
            err.code as i32
        );

        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("disk_cached.txt", vec![7u8; 4096])).unwrap();
        }
        let path = CString::new("disk_cached.txt").unwrap();
        for _ in 0..3 {
            let mut serr = OdError::ok();
            let r = unsafe { od_reader_open(op, path.as_ptr(), &mut serr) };
            assert!(!r.is_null());
            let mut buf = vec![0u8; 4096];
            let n = unsafe { od_reader_read(r, 0, 4096, buf.as_mut_ptr(), &mut serr) };
            assert_eq!(n, 4096);
            assert_eq!(buf[0], 7);
            unsafe { od_reader_free(r) };
        }

        unsafe { od_operator_free(op) };

        // The on-disk cache directory should contain foyer's data files.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!entries.is_empty(), "foyer disk cache dir is empty");
    }

    #[test]
    fn capability_list_and_rename_guard() {
        // memory supports read/write/list but NOT server-side rename, so the
        // capability list must reflect that and the rename guard must fail-fast.
        let scheme = CString::new("memory").unwrap();
        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(!op.is_null());

        // Capability list: collect into a name→supported map.
        let mut cerr = OdError::ok();
        let list = unsafe { od_capabilities(op, &mut cerr) };
        assert!(!list.is_null());
        let n = unsafe { od_capabilities_len(list) };
        assert!(n > 0);
        let mut caps = std::collections::HashMap::new();
        for i in 0..n {
            let mut ent = OdCapability {
                name: std::ptr::null(),
                supported: 0,
            };
            assert_eq!(unsafe { od_capabilities_entry(list, i, &mut ent) }, 1);
            let name = unsafe { CStr::from_ptr(ent.name) }
                .to_str()
                .unwrap()
                .to_owned();
            caps.insert(name, ent.supported == 1);
        }
        unsafe { od_capabilities_free(list) };
        assert_eq!(caps.get("read"), Some(&true));
        assert_eq!(caps.get("write"), Some(&true));
        assert_eq!(caps.get("list"), Some(&true));
        assert_eq!(caps.get("rename"), Some(&false));
        assert_eq!(caps.get("copy"), Some(&false));

        // od_operator_supports reads the same cached cap without materializing
        // a list — must agree with the list above.
        let probe = |name: &str| {
            let c = CString::new(name).unwrap();
            let rc = unsafe { crate::od_operator_supports(op, c.as_ptr()) };
            rc == 1
        };
        assert!(probe("read"));
        assert!(probe("write"));
        assert!(!probe("rename"));
        assert!(!probe("copy"));
        assert!(!probe("definitely_not_a_capability"));

        // rename must fail-fast with Unsupported + a clear message, without
        // touching the backend.
        let from = CString::new("a.txt").unwrap();
        let to = CString::new("b.txt").unwrap();
        let mut merr = OdError::ok();
        let rc = unsafe { od_rename(op, from.as_ptr(), to.as_ptr(), &mut merr) };
        assert_eq!(rc, -1);
        assert_eq!(merr.code as i32, OdErrorCode::Unsupported as i32);
        let msg = unsafe { CStr::from_ptr(merr.message) }.to_str().unwrap();
        assert!(
            msg.contains("memory") && msg.contains("rename"),
            "unexpected msg: {msg}"
        );
        unsafe { od_string_free(merr.message) };

        // copy likewise fail-fasts on memory (no copy capability).
        let mut cerr = OdError::ok();
        let rc = unsafe { od_copy(op, from.as_ptr(), to.as_ptr(), &mut cerr) };
        assert_eq!(rc, -1);
        assert_eq!(cerr.code as i32, OdErrorCode::Unsupported as i32);
        unsafe { od_string_free(cerr.message) };

        unsafe { od_operator_free(op) };
    }

    #[test]
    fn fs_copy_succeeds() {
        // The fs service supports copy; exercise the od_copy FFI end to end
        // (this is the branch the C++ MoveFile copy+delete fallback uses when a
        // service lacks server-side rename, e.g. s3).
        init(); // register services (no auto-register ctor)
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        let uri = CString::new("fs://").unwrap();
        let keys = [CString::new("root").unwrap()];
        let vals = [CString::new(root).unwrap()];
        let k_ptrs: Vec<*const c_char> = keys.iter().map(|c| c.as_ptr()).collect();
        let v_ptrs: Vec<*const c_char> = vals.iter().map(|c| c.as_ptr()).collect();
        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                uri.as_ptr(),
                k_ptrs.as_ptr(),
                v_ptrs.as_ptr(),
                k_ptrs.len(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(!op.is_null(), "fs operator_new failed: {}", err.code as i32);

        // Seed a file, copy it, verify both exist with the same content.
        {
            let inner = unsafe { &(*op).op };
            crate::runtime::block_on(inner.write("src.txt", b"copy me".to_vec())).unwrap();
        }
        let from = CString::new("src.txt").unwrap();
        let to = CString::new("dst.txt").unwrap();
        let mut cerr = OdError::ok();
        assert_eq!(
            unsafe { od_copy(op, from.as_ptr(), to.as_ptr(), &mut cerr) },
            0
        );

        let mut e = OdError::ok();
        assert_eq!(unsafe { od_exists(op, from.as_ptr(), &mut e) }, 1);
        assert_eq!(unsafe { od_exists(op, to.as_ptr(), &mut e) }, 1);

        unsafe { od_operator_free(op) };
    }

    #[test]
    fn memory_operator_with_io_options() {
        // Set per-operator I/O tuning via the layers map (io.* keys) and confirm
        // read/write still work — the options are transparent to correctness.
        let scheme = CString::new("memory").unwrap();
        let lk: Vec<CString> = [
            "io.write.concurrent",
            "io.write.chunk_size",
            "io.read.concurrent",
            "io.read.chunk_size",
        ]
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
        let lv: Vec<CString> = ["4", "1048576", "2", "262144"]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
        let lk_ptrs: Vec<*const c_char> = lk.iter().map(|c| c.as_ptr()).collect();
        let lv_ptrs: Vec<*const c_char> = lv.iter().map(|c| c.as_ptr()).collect();

        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                scheme.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                lk_ptrs.as_ptr(),
                lv_ptrs.as_ptr(),
                lk_ptrs.len(),
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(
            !op.is_null(),
            "io-options operator_new failed: {}",
            err.code as i32
        );
        // The parsed options are stored on the operator.
        unsafe {
            assert_eq!((*op).io.write.concurrent.map(|value| value.get()), Some(4));
            assert_eq!((*op).io.write_chunk(), Some(1048576));
            assert_eq!((*op).io.read.concurrent.map(|value| value.get()), Some(2));
            assert_eq!((*op).io.read_chunk(), Some(262144));
        }

        // Round-trip a write + read through the tuned writer/reader.
        let path = CString::new("tuned.txt").unwrap();
        let mut werr = OdError::ok();
        let w = unsafe { od_writer_open(op, path.as_ptr(), &mut werr) };
        assert!(!w.is_null());
        let payload = b"tuned io";
        assert_eq!(
            unsafe { od_writer_write(w, payload.as_ptr(), payload.len() as u64, &mut werr) },
            0
        );
        assert_eq!(unsafe { od_writer_close(w, &mut werr) }, 0);
        unsafe { od_writer_free(w) };

        let mut rerr = OdError::ok();
        let r = unsafe { od_reader_open(op, path.as_ptr(), &mut rerr) };
        assert!(!r.is_null());
        let mut buf = vec![0u8; payload.len()];
        let n = unsafe { od_reader_read(r, 0, payload.len() as u64, buf.as_mut_ptr(), &mut rerr) };
        assert_eq!(n as usize, payload.len());
        assert_eq!(&buf, payload);
        unsafe { od_reader_free(r) };

        unsafe { od_operator_free(op) };
    }

    #[test]
    fn operator_rejects_invalid_cross_service_options() {
        let build = |key: &str, value: &str| {
            let uri = CString::new("memory://").unwrap();
            let key = CString::new(key).unwrap();
            let value = CString::new(value).unwrap();
            let keys = [key.as_ptr()];
            let values = [value.as_ptr()];
            let mut err = OdError::ok();
            let op = unsafe {
                od_operator_new(
                    uri.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    std::ptr::null(),
                    &mut err,
                )
            };
            (op, err)
        };

        for (key, value) in [
            ("retry.max_times", "banana"),
            ("retry.factor", "0.5"),
            ("retry.jitter", "yes"),
            ("io.read.concurrent", "0"),
            ("cache.shards", "0"),
            ("cache.unknown", "1"),
        ] {
            let (op, err) = build(key, value);
            assert!(op.is_null(), "accepted {key}={value}");
            assert_eq!(err.code as i32, OdErrorCode::InvalidInput as i32);
            unsafe { od_string_free(err.message) };
        }
    }

    #[test]
    fn service_config_validation_is_propagated() {
        init();
        let uri = CString::new("s3://bucket").unwrap();
        let key = CString::new("enable_virtual_host_style").unwrap();
        let value = CString::new("banana").unwrap();
        let keys = [key.as_ptr()];
        let values = [value.as_ptr()];
        let mut err = OdError::ok();
        let op = unsafe {
            od_operator_new(
                uri.as_ptr(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(op.is_null());
        assert_eq!(err.code as i32, OdErrorCode::InvalidInput as i32);
        unsafe { od_string_free(err.message) };
    }

    #[test]
    fn scheme_membership_reads_registry() {
        init(); // populate the registry
        let sup = |s: &str| {
            let c = CString::new(s).unwrap();
            (unsafe { od_scheme_supported(c.as_ptr()) }) == 1
        };
        // Baseline compiled-in services.
        assert!(sup("memory"));
        assert!(sup("fs"));
        assert!(sup("s3"));
        assert!(!sup("definitely_not_a_scheme"));

        let mut err = OdError::ok();
        let list = unsafe { od_schemes(&mut err) };
        assert!(!list.is_null());
        let schemes = (0..unsafe { od_schemes_len(list) })
            .map(|index| {
                unsafe { CStr::from_ptr(od_schemes_entry(list, index)) }
                    .to_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert!(schemes.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(schemes.iter().all(|scheme| sup(scheme)));
        assert!(schemes.iter().any(|scheme| scheme == "memory"));
        unsafe { od_schemes_free(list) };
    }
}
