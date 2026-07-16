//! Directory listing across the FFI boundary.
//!
//! `od_list` materializes the entries under a path into an opaque
//! `OdEntryList`. The C++ side reads entries by index and frees the list with
//! `od_list_free`. OpenDAL's `list_with(path).recursive(..)` already collects
//! into a `Vec<Entry>`, so a materialized list is the simplest correct FFI for
//! `ls` / `du` / glob. (A streaming cursor can be added later if huge directory
//! listings need it.)

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::future::IntoFuture;

use opendal::Entry;

use crate::capability::require;
use crate::error::{set_error, set_ok, set_opendal_error, OdError, OdErrorCode};
use crate::ffi::{cstr, ffi_guard, free_handle};
use crate::operator::OdOperator;
use crate::runtime::block_on;

/// Opaque, index-addressable list of entries.
///
/// `Entry::path()`/`name()` are non-NUL-terminated `&str`, so NUL-terminated
/// `CString`s are built and cached lazily (list-lifetime) on first access. The
/// cache is an `UnsafeCell` (entry access takes `&self`); this is sound only
/// because a list never escapes the function that created it — every `od_list`
/// caller (`Glob`/`ListFiles`/`ls`/`du`) creates, consumes, and frees it in one
/// scope, so the cache is never touched concurrently. Do not stash an
/// `OdEntryList` in shared/scan state without a thread-safe cache.
pub struct OdEntryList {
    entries: Vec<Entry>,
    strings: UnsafeCell<HashMap<usize, (CString, CString)>>,
}

/// One entry's metadata, returned by value from `od_list_entry`.
///
/// `path` and `name` are borrowed pointers into the `OdEntryList` and are
/// valid only until the list is freed. The C++ side must copy them out.
#[repr(C)]
pub struct OdEntry {
    /// Full path of the entry (relative to the operator root), NUL-terminated.
    /// Borrowed from the list; do NOT free.
    pub path: *const c_char,
    /// Base name of the entry, NUL-terminated. Borrowed; do NOT free.
    pub name: *const c_char,
    pub content_length: u64,
    pub last_modified_ms: i64,
    pub is_dir: u8,
}

impl OdEntry {
    fn empty() -> Self {
        OdEntry {
            path: std::ptr::null(),
            name: std::ptr::null(),
            content_length: 0,
            last_modified_ms: -1,
            is_dir: 0,
        }
    }
}

/// Build (once) and return borrowed C-string pointers for entry `index`.
unsafe fn cached_ptrs(list: &OdEntryList, index: usize) -> (*const c_char, *const c_char) {
    let cache = &mut *list.strings.get();
    let (p, n) = cache.entry(index).or_insert_with(|| {
        let e = &list.entries[index];
        (
            CString::new(e.path()).unwrap_or_default(),
            CString::new(e.name()).unwrap_or_default(),
        )
    });
    (p.as_ptr(), n.as_ptr())
}

/// List entries under `path`. When `recursive` is non-zero, descends into
/// subdirectories.
///
/// On success returns a non-null `*mut OdEntryList` and sets `*err` to Ok.
///
/// # Safety
/// - `op` must be a live handle from `od_operator_new`.
/// - `path` must be a valid NUL-terminated C string.
/// - The returned handle must be freed once with `od_list_free`.
#[no_mangle]
pub unsafe extern "C" fn od_list(
    op: *const OdOperator,
    path: *const c_char,
    recursive: u8,
    err: *mut OdError,
) -> *mut OdEntryList {
    ffi_guard!(err, std::ptr::null_mut(), "od_list", {
        if op.is_null() || path.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null operator or path");
            return std::ptr::null_mut();
        }
        let odop = &*op;
        if let Err((code, msg)) = require(&odop.scheme, odop.cap.list, "list") {
            set_error(err, code, msg);
            return std::ptr::null_mut();
        }
        // NB: recursion is provided by OpenDAL's raw layer even when a backend
        // does not advertise `list_with_recursive`, so we guard only `list`.
        let path = match cstr(path) {
            Some(s) => s,
            None => {
                set_error(err, OdErrorCode::InvalidInput, "path is null or not UTF-8");
                return std::ptr::null_mut();
            }
        };

        match block_on(
            odop.op
                .list_with(path)
                .recursive(recursive != 0)
                .into_future(),
        ) {
            Ok(entries) => {
                set_ok(err);
                Box::into_raw(Box::new(OdEntryList {
                    entries,
                    strings: UnsafeCell::new(HashMap::new()),
                }))
            }
            Err(e) => {
                set_opendal_error(err, &e);
                std::ptr::null_mut()
            }
        }
    })
}

/// Number of entries in the list. Returns 0 if `list` is null.
///
/// # Safety
/// `list` must be null or a live handle from `od_list`.
#[no_mangle]
pub unsafe extern "C" fn od_list_len(list: *const OdEntryList) -> usize {
    if list.is_null() {
        return 0;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (*list).entries.len())).unwrap_or(0)
}

/// Fetch entry `index` into `out`. `path`/`name` in `out` borrow the list and
/// are valid until `od_list_free`. Returns 1 on success, 0 if out of range.
///
/// # Safety
/// - `list` must be a live handle from `od_list`.
/// - `out` must be a valid, writable pointer.
/// - The returned `path`/`name` pointers must not be used after the list is
///   freed, and must not be freed themselves.
#[no_mangle]
pub unsafe extern "C" fn od_list_entry(
    list: *const OdEntryList,
    index: usize,
    out: *mut OdEntry,
    err: *mut OdError,
) -> u8 {
    ffi_guard!(err, 0, "od_list_entry", {
        if !out.is_null() {
            *out = OdEntry::empty();
        }
        if list.is_null() || out.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null list or out parameter");
            return 0;
        }
        let list_ref = &(*list);
        if index >= list_ref.entries.len() {
            set_error(err, OdErrorCode::InvalidInput, "index out of bounds");
            return 0;
        }
        let (path_ptr, name_ptr) = cached_ptrs(list_ref, index);
        let meta = list_ref.entries[index].metadata();
        let last_modified_ms = meta
            .last_modified()
            .map(|t| t.into_inner().as_millisecond())
            .unwrap_or(-1);
        *out = OdEntry {
            path: path_ptr,
            name: name_ptr,
            content_length: meta.content_length(),
            last_modified_ms,
            is_dir: if meta.is_dir() { 1 } else { 0 },
        };
        set_ok(err);
        1
    })
}

/// Free an entry list. Safe to call with null (no-op).
///
/// # Safety
/// `list` must be null or a handle from `od_list`, not already freed, with no
/// borrowed `path`/`name` pointers still in use.
#[no_mangle]
pub unsafe extern "C" fn od_list_free(list: *mut OdEntryList) {
    free_handle(list);
}
