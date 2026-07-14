//! Rust-owned table scans for OpenDAL entries and metadata.

use std::collections::HashMap;
use std::ffi::{c_char, CString};

use futures::TryStreamExt;
use opendal::options::{CopyOptions, ListOptions, ReaderOptions, StatOptions, WriteOptions};
use opendal::{Entry, EntryMode, Lister, Metadata};

use crate::capability::require;
use crate::error::{set_error, set_ok, set_opendal_error, OdError, OdErrorCode};
use crate::ffi::{cstr, ffi_guard, free_handle};
use crate::operator::OdOperator;
use crate::runtime::block_on;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdStatOptions {
    pub version: *const c_char,
    pub if_match: *const c_char,
    pub if_none_match: *const c_char,
    pub if_modified_since_ms: i64,
    pub has_if_modified_since: u8,
    pub if_unmodified_since_ms: i64,
    pub has_if_unmodified_since: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdListOptions {
    pub limit: usize,
    pub has_limit: u8,
    pub start_after: *const c_char,
    pub recursive: u8,
    pub versions: u8,
    pub deleted: u8,
}

#[repr(C)]
pub struct OdEntryMetadata {
    pub mode: u8,
    pub content_length: u64,
    pub cache_control: *const c_char,
    pub content_disposition: *const c_char,
    pub content_md5: *const c_char,
    pub content_type: *const c_char,
    pub content_encoding: *const c_char,
    pub etag: *const c_char,
    pub last_modified_ms: i64,
    pub has_last_modified: u8,
    pub version: *const c_char,
    pub is_current: u8,
    pub has_is_current: u8,
    pub is_deleted: u8,
    pub user_metadata_keys: *const *const c_char,
    pub user_metadata_values: *const *const c_char,
    pub user_metadata_len: usize,
}

#[repr(C)]
pub struct OdEntryRow {
    pub path: *const c_char,
    pub name: *const c_char,
    pub metadata: OdEntryMetadata,
}

#[repr(C)]
pub struct OdDuRow {
    pub directory: *const c_char,
    pub file_count: u64,
    pub total_size: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdCopyOptions {
    pub if_not_exists: u8,
    pub if_match: *const c_char,
    pub source_version: *const c_char,
    pub source_content_length_hint: u64,
    pub has_source_content_length_hint: u8,
    pub concurrent: usize,
    pub has_concurrent: u8,
    pub chunk_size: usize,
    pub has_chunk_size: u8,
}

#[repr(C)]
pub struct OdCopyRow {
    pub bytes_copied: u64,
    pub metadata: OdEntryMetadata,
}

struct OwnedDuRow {
    directory: CString,
    file_count: u64,
    total_size: u64,
}

pub struct OdDuCursor {
    rows: std::vec::IntoIter<OwnedDuRow>,
    current: Option<OwnedDuRow>,
}

pub struct OdCopyCursor {
    row: Option<(u64, OwnedRow)>,
    current: Option<(u64, OwnedRow)>,
}

struct OwnedRow {
    path: CString,
    name: CString,
    cache_control: Option<CString>,
    content_disposition: Option<CString>,
    content_md5: Option<CString>,
    content_type: Option<CString>,
    content_encoding: Option<CString>,
    etag: Option<CString>,
    version: Option<CString>,
    _user_keys: Vec<CString>,
    _user_values: Vec<CString>,
    user_key_ptrs: Vec<*const c_char>,
    user_value_ptrs: Vec<*const c_char>,
    mode: u8,
    content_length: u64,
    last_modified_ms: i64,
    has_last_modified: u8,
    is_current: u8,
    has_is_current: u8,
    is_deleted: u8,
}

enum CursorSource {
    One(Option<(String, Metadata)>),
    List(Lister),
}

pub struct OdTableCursor {
    source: CursorSource,
    glob: Option<String>,
    self_path: Option<String>,
    current: Option<OwnedRow>,
}

fn optional_cstr(value: *const c_char, name: &str) -> Result<Option<String>, String> {
    if value.is_null() {
        Ok(None)
    } else {
        unsafe { cstr(value) }
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| format!("{name} is not UTF-8"))
    }
}

fn timestamp(ms: i64, present: u8, name: &str) -> Result<Option<opendal::raw::Timestamp>, String> {
    if present == 0 {
        return Ok(None);
    }
    opendal::raw::Timestamp::from_millisecond(ms)
        .map(Some)
        .map_err(|error| format!("invalid {name}: {error}"))
}

fn stat_options(raw: OdStatOptions) -> Result<StatOptions, String> {
    Ok(StatOptions {
        version: optional_cstr(raw.version, "version")?,
        if_match: optional_cstr(raw.if_match, "if_match")?,
        if_none_match: optional_cstr(raw.if_none_match, "if_none_match")?,
        if_modified_since: timestamp(
            raw.if_modified_since_ms,
            raw.has_if_modified_since,
            "if_modified_since",
        )?,
        if_unmodified_since: timestamp(
            raw.if_unmodified_since_ms,
            raw.has_if_unmodified_since,
            "if_unmodified_since",
        )?,
        ..Default::default()
    })
}

fn list_options(raw: OdListOptions) -> Result<ListOptions, String> {
    Ok(ListOptions {
        limit: (raw.has_limit != 0).then_some(raw.limit),
        start_after: optional_cstr(raw.start_after, "start_after")?,
        recursive: raw.recursive != 0,
        versions: raw.versions != 0,
        deleted: raw.deleted != 0,
    })
}

fn cstring(value: &str, field: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| format!("OpenDAL {field} contains a NUL byte"))
}

fn optional_string(value: Option<&str>, field: &str) -> Result<Option<CString>, String> {
    value.map(|value| cstring(value, field)).transpose()
}

impl OwnedRow {
    fn new(entry: Entry) -> Result<Self, String> {
        let (path, metadata) = entry.into_parts();
        Self::from_parts(path, metadata)
    }

    fn from_parts(path: String, metadata: Metadata) -> Result<Self, String> {
        let name = opendal::raw::get_basename(&path);
        let user_metadata = metadata.user_metadata().cloned().unwrap_or_default();
        let mut user_metadata: Vec<_> = user_metadata.into_iter().collect();
        user_metadata.sort_by(|a, b| a.0.cmp(&b.0));
        let mut user_keys = Vec::with_capacity(user_metadata.len());
        let mut user_values = Vec::with_capacity(user_metadata.len());
        for (key, value) in user_metadata {
            user_keys.push(cstring(&key, "user metadata key")?);
            user_values.push(cstring(&value, "user metadata value")?);
        }
        let user_key_ptrs = user_keys.iter().map(|value| value.as_ptr()).collect();
        let user_value_ptrs = user_values.iter().map(|value| value.as_ptr()).collect();
        let last_modified = metadata.last_modified();
        let is_current = metadata.is_current();
        Ok(Self {
            path: cstring(&path, "path")?,
            name: cstring(name, "name")?,
            cache_control: optional_string(metadata.cache_control(), "cache_control")?,
            content_disposition: optional_string(
                metadata.content_disposition(),
                "content_disposition",
            )?,
            content_md5: optional_string(metadata.content_md5(), "content_md5")?,
            content_type: optional_string(metadata.content_type(), "content_type")?,
            content_encoding: optional_string(metadata.content_encoding(), "content_encoding")?,
            etag: optional_string(metadata.etag(), "etag")?,
            version: optional_string(metadata.version(), "version")?,
            _user_keys: user_keys,
            _user_values: user_values,
            user_key_ptrs,
            user_value_ptrs,
            mode: match metadata.mode() {
                EntryMode::FILE => 1,
                EntryMode::DIR => 2,
                _ => 0,
            },
            content_length: metadata.content_length(),
            last_modified_ms: last_modified
                .map(|value| value.into_inner().as_millisecond())
                .unwrap_or_default(),
            has_last_modified: last_modified.is_some() as u8,
            is_current: is_current.unwrap_or_default() as u8,
            has_is_current: is_current.is_some() as u8,
            is_deleted: metadata.is_deleted() as u8,
        })
    }

    fn borrowed(&self) -> OdEntryRow {
        let ptr = |value: &Option<CString>| {
            value
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr())
        };
        OdEntryRow {
            path: self.path.as_ptr(),
            name: self.name.as_ptr(),
            metadata: OdEntryMetadata {
                mode: self.mode,
                content_length: self.content_length,
                cache_control: ptr(&self.cache_control),
                content_disposition: ptr(&self.content_disposition),
                content_md5: ptr(&self.content_md5),
                content_type: ptr(&self.content_type),
                content_encoding: ptr(&self.content_encoding),
                etag: ptr(&self.etag),
                last_modified_ms: self.last_modified_ms,
                has_last_modified: self.has_last_modified,
                version: ptr(&self.version),
                is_current: self.is_current,
                has_is_current: self.has_is_current,
                is_deleted: self.is_deleted,
                user_metadata_keys: self.user_key_ptrs.as_ptr(),
                user_metadata_values: self.user_value_ptrs.as_ptr(),
                user_metadata_len: self.user_key_ptrs.len(),
            },
        }
    }
}

fn glob_matches(pattern: &[u8], value: &[u8]) -> bool {
    fn run(pattern: &[u8], value: &[u8], memo: &mut HashMap<(usize, usize), bool>) -> bool {
        let key = (pattern.len(), value.len());
        if let Some(result) = memo.get(&key) {
            return *result;
        }
        let result = if pattern.is_empty() {
            value.is_empty()
        } else if pattern.starts_with(b"**") {
            (pattern.get(2) == Some(&b'/') && run(&pattern[3..], value, memo))
                || run(&pattern[2..], value, memo)
                || (!value.is_empty() && run(pattern, &value[1..], memo))
        } else if pattern[0] == b'*' {
            run(&pattern[1..], value, memo)
                || (!value.is_empty() && value[0] != b'/' && run(pattern, &value[1..], memo))
        } else if pattern[0] == b'?' {
            !value.is_empty() && value[0] != b'/' && run(&pattern[1..], &value[1..], memo)
        } else if pattern[0] == b'[' {
            let Some(end) = pattern.iter().position(|value| *value == b']') else {
                return false;
            };
            if value.is_empty() || value[0] == b'/' {
                false
            } else {
                let class = &pattern[1..end];
                let negated = class
                    .first()
                    .is_some_and(|value| *value == b'!' || *value == b'^');
                let class = if negated { &class[1..] } else { class };
                let mut matched = false;
                let mut index = 0;
                while index < class.len() {
                    if index + 2 < class.len() && class[index + 1] == b'-' {
                        matched |= class[index] <= value[0] && value[0] <= class[index + 2];
                        index += 3;
                    } else {
                        matched |= class[index] == value[0];
                        index += 1;
                    }
                }
                matched != negated && run(&pattern[end + 1..], &value[1..], memo)
            }
        } else {
            !value.is_empty() && pattern[0] == value[0] && run(&pattern[1..], &value[1..], memo)
        };
        memo.insert(key, result);
        result
    }
    run(pattern, value, &mut HashMap::new())
}

#[no_mangle]
pub unsafe extern "C" fn od_table_stat_open(
    op: *const OdOperator,
    path: *const c_char,
    raw_options: OdStatOptions,
    err: *mut OdError,
) -> *mut OdTableCursor {
    ffi_guard!(err, std::ptr::null_mut(), "od_table_stat_open", {
        if op.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null operator");
            return std::ptr::null_mut();
        }
        let path = match cstr(path) {
            Some(path) => path,
            None => {
                set_error(err, OdErrorCode::InvalidInput, "path is null or not UTF-8");
                return std::ptr::null_mut();
            }
        };
        let odop = &*op;
        if let Err((code, message)) = require(&odop.scheme, odop.cap.stat, "stat") {
            set_error(err, code, message);
            return std::ptr::null_mut();
        }
        let options = match stat_options(raw_options) {
            Ok(options) => options,
            Err(message) => {
                set_error(err, OdErrorCode::InvalidInput, message);
                return std::ptr::null_mut();
            }
        };
        match block_on(odop.op.stat_options(path, options)) {
            Ok(metadata) => {
                set_ok(err);
                Box::into_raw(Box::new(OdTableCursor {
                    source: CursorSource::One(Some((path.to_owned(), metadata))),
                    glob: None,
                    self_path: None,
                    current: None,
                }))
            }
            Err(error) => {
                set_opendal_error(err, &error);
                std::ptr::null_mut()
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_table_list_open(
    op: *const OdOperator,
    path: *const c_char,
    glob: *const c_char,
    raw_options: OdListOptions,
    err: *mut OdError,
) -> *mut OdTableCursor {
    ffi_guard!(err, std::ptr::null_mut(), "od_table_list_open", {
        if op.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null operator");
            return std::ptr::null_mut();
        }
        let path = match cstr(path) {
            Some(path) => path,
            None => {
                set_error(err, OdErrorCode::InvalidInput, "path is null or not UTF-8");
                return std::ptr::null_mut();
            }
        };
        let glob = match optional_cstr(glob, "glob") {
            Ok(glob) => glob,
            Err(message) => {
                set_error(err, OdErrorCode::InvalidInput, message);
                return std::ptr::null_mut();
            }
        };
        let odop = &*op;
        if let Err((code, message)) = require(&odop.scheme, odop.cap.list, "list") {
            set_error(err, code, message);
            return std::ptr::null_mut();
        }
        let options = match list_options(raw_options) {
            Ok(options) => options,
            Err(message) => {
                set_error(err, OdErrorCode::InvalidInput, message);
                return std::ptr::null_mut();
            }
        };
        match block_on(odop.op.lister_options(path, options)) {
            Ok(lister) => {
                set_ok(err);
                Box::into_raw(Box::new(OdTableCursor {
                    source: CursorSource::List(lister),
                    glob,
                    self_path: Some(path.trim_matches('/').to_owned()),
                    current: None,
                }))
            }
            Err(error) => {
                set_opendal_error(err, &error);
                std::ptr::null_mut()
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_table_glob_open(
    op: *const OdOperator,
    pattern: *const c_char,
    mut raw_options: OdListOptions,
    err: *mut OdError,
) -> *mut OdTableCursor {
    ffi_guard!(err, std::ptr::null_mut(), "od_table_glob_open", {
        let pattern = match cstr(pattern) {
            Some(pattern) => pattern,
            None => {
                set_error(
                    err,
                    OdErrorCode::InvalidInput,
                    "pattern is null or not UTF-8",
                );
                return std::ptr::null_mut();
            }
        };
        let wildcard = pattern
            .bytes()
            .position(|value| matches!(value, b'*' | b'?' | b'['));
        let Some(wildcard) = wildcard else {
            set_error(
                err,
                OdErrorCode::InvalidInput,
                "glob pattern has no wildcard",
            );
            return std::ptr::null_mut();
        };
        let list_root = pattern[..wildcard]
            .rfind('/')
            .map_or("", |slash| &pattern[..=slash]);
        raw_options.recursive = 1;
        let normalized_pattern = pattern.trim_start_matches('/');
        let pattern = match CString::new(normalized_pattern) {
            Ok(pattern) => pattern,
            Err(_) => {
                set_error(
                    err,
                    OdErrorCode::InvalidInput,
                    "glob pattern contains a NUL byte",
                );
                return std::ptr::null_mut();
            }
        };
        let list_root = match CString::new(list_root) {
            Ok(list_root) => list_root,
            Err(_) => unreachable!(),
        };
        od_table_list_open(op, list_root.as_ptr(), pattern.as_ptr(), raw_options, err)
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_table_cursor_next(
    cursor: *mut OdTableCursor,
    out: *mut OdEntryRow,
    err: *mut OdError,
) -> i8 {
    ffi_guard!(err, -1, "od_table_cursor_next", {
        if cursor.is_null() || out.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null cursor or row output");
            return -1;
        }
        let cursor = &mut *cursor;
        loop {
            let row = match &mut cursor.source {
                CursorSource::One(entry) => entry
                    .take()
                    .map(|(path, metadata)| OwnedRow::from_parts(path, metadata)),
                CursorSource::List(lister) => match block_on(lister.try_next()) {
                    Ok(entry) => entry.map(OwnedRow::new),
                    Err(error) => {
                        set_opendal_error(err, &error);
                        return -1;
                    }
                },
            };
            let Some(row) = row else {
                set_ok(err);
                return 0;
            };
            match row {
                Ok(row) => {
                    if cursor.self_path.as_ref().is_some_and(|self_path| {
                        row.path
                            .to_bytes()
                            .iter()
                            .copied()
                            .eq(self_path.as_bytes().iter().copied())
                            || row
                                .path
                                .to_bytes()
                                .strip_suffix(b"/")
                                .is_some_and(|path| path == self_path.as_bytes())
                    }) {
                        continue;
                    }
                    if cursor.glob.as_ref().is_some_and(|pattern| {
                        !glob_matches(pattern.as_bytes(), row.path.as_bytes())
                    }) {
                        continue;
                    }
                    cursor.current = Some(row);
                    *out = cursor.current.as_ref().unwrap().borrowed();
                    set_ok(err);
                    return 1;
                }
                Err(message) => {
                    set_error(err, OdErrorCode::Unexpected, message);
                    return -1;
                }
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_table_cursor_free(cursor: *mut OdTableCursor) {
    free_handle(cursor);
}

fn has_glob(path: &str) -> bool {
    path.bytes()
        .any(|value| matches!(value, b'*' | b'?' | b'['))
}

fn glob_root(pattern: &str) -> &str {
    let wildcard = pattern
        .bytes()
        .position(|value| matches!(value, b'*' | b'?' | b'['))
        .unwrap_or(pattern.len());
    pattern[..wildcard]
        .rfind('/')
        .map_or("", |slash| &pattern[..=slash])
}

fn parent(path: &str) -> &str {
    path.trim_end_matches('/')
        .rfind('/')
        .map_or("", |slash| &path[..slash])
}

#[no_mangle]
pub unsafe extern "C" fn od_table_du_open(
    op: *const OdOperator,
    target: *const c_char,
    mut raw_options: OdListOptions,
    err: *mut OdError,
) -> *mut OdDuCursor {
    ffi_guard!(err, std::ptr::null_mut(), "od_table_du_open", {
        if op.is_null() {
            set_error(err, OdErrorCode::InvalidInput, "null operator");
            return std::ptr::null_mut();
        }
        let target = match cstr(target) {
            Some(target) => target,
            None => {
                set_error(
                    err,
                    OdErrorCode::InvalidInput,
                    "target is null or not UTF-8",
                );
                return std::ptr::null_mut();
            }
        };
        let odop = &*op;
        let options = match list_options(raw_options) {
            Ok(options) => options,
            Err(message) => {
                set_error(err, OdErrorCode::InvalidInput, message);
                return std::ptr::null_mut();
            }
        };
        let entries: Result<Vec<(String, Metadata)>, opendal::Error> = if has_glob(target) {
            raw_options.recursive = 1;
            match block_on(odop.op.list_options(
                glob_root(target),
                ListOptions {
                    recursive: true,
                    ..options
                },
            )) {
                Ok(entries) => Ok(entries
                    .into_iter()
                    .filter(|entry| {
                        glob_matches(
                            target.trim_start_matches('/').as_bytes(),
                            entry.path().as_bytes(),
                        )
                    })
                    .map(Entry::into_parts)
                    .collect()),
                Err(error) => Err(error),
            }
        } else {
            match block_on(odop.op.stat(target)) {
                Ok(metadata) if metadata.is_file() => Ok(vec![(target.to_owned(), metadata)]),
                Ok(_) => {
                    let directory = if target.ends_with('/') {
                        target.to_owned()
                    } else {
                        format!("{target}/")
                    };
                    block_on(odop.op.list_options(
                        &directory,
                        ListOptions {
                            recursive: true,
                            ..options
                        },
                    ))
                    .map(|entries| entries.into_iter().map(Entry::into_parts).collect())
                }
                Err(error) => Err(error),
            }
        };
        let entries = match entries {
            Ok(entries) => entries,
            Err(error) => {
                set_opendal_error(err, &error);
                return std::ptr::null_mut();
            }
        };
        let mut groups: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
        for (path, metadata) in entries {
            if !metadata.is_file() {
                continue;
            }
            let group = groups.entry(parent(&path).to_owned()).or_default();
            group.0 = match group.0.checked_add(1) {
                Some(value) => value,
                None => {
                    set_error(err, OdErrorCode::Unexpected, "du file count overflow");
                    return std::ptr::null_mut();
                }
            };
            group.1 = match group.1.checked_add(metadata.content_length()) {
                Some(value) => value,
                None => {
                    set_error(err, OdErrorCode::Unexpected, "du byte count overflow");
                    return std::ptr::null_mut();
                }
            };
        }
        let mut rows = Vec::with_capacity(groups.len());
        for (directory, (file_count, total_size)) in groups {
            let directory = match cstring(&directory, "du directory") {
                Ok(directory) => directory,
                Err(message) => {
                    set_error(err, OdErrorCode::Unexpected, message);
                    return std::ptr::null_mut();
                }
            };
            rows.push(OwnedDuRow {
                directory,
                file_count,
                total_size,
            });
        }
        set_ok(err);
        Box::into_raw(Box::new(OdDuCursor {
            rows: rows.into_iter(),
            current: None,
        }))
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_du_cursor_next(
    cursor: *mut OdDuCursor,
    out: *mut OdDuRow,
    err: *mut OdError,
) -> i8 {
    ffi_guard!(err, -1, "od_du_cursor_next", {
        if cursor.is_null() || out.is_null() {
            set_error(
                err,
                OdErrorCode::InvalidInput,
                "null du cursor or row output",
            );
            return -1;
        }
        let cursor = &mut *cursor;
        let Some(row) = cursor.rows.next() else {
            set_ok(err);
            return 0;
        };
        cursor.current = Some(row);
        let row = cursor.current.as_ref().unwrap();
        *out = OdDuRow {
            directory: row.directory.as_ptr(),
            file_count: row.file_count,
            total_size: row.total_size,
        };
        set_ok(err);
        1
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_du_cursor_free(cursor: *mut OdDuCursor) {
    free_handle(cursor);
}

fn copy_options(raw: OdCopyOptions) -> Result<CopyOptions, String> {
    if raw.has_concurrent != 0 && raw.concurrent == 0 {
        return Err("copy concurrent must be positive".to_string());
    }
    if raw.has_chunk_size != 0 && raw.chunk_size == 0 {
        return Err("copy chunk_size must be positive".to_string());
    }
    Ok(CopyOptions {
        if_not_exists: raw.if_not_exists != 0,
        if_match: optional_cstr(raw.if_match, "if_match")?,
        source_version: optional_cstr(raw.source_version, "source_version")?,
        source_content_length_hint: (raw.has_source_content_length_hint != 0)
            .then_some(raw.source_content_length_hint),
        concurrent: if raw.has_concurrent != 0 {
            raw.concurrent
        } else {
            0
        },
        chunk: (raw.has_chunk_size != 0).then_some(raw.chunk_size),
    })
}

fn abort_writer(writer: &mut opendal::Writer, primary: opendal::Error) -> opendal::Error {
    match block_on(writer.abort()) {
        Ok(()) => primary,
        Err(abort) => primary.with_context("abort_error", abort.to_string()),
    }
}

#[no_mangle]
pub unsafe extern "C" fn od_table_copy_open(
    source_op: *const OdOperator,
    source: *const c_char,
    destination_op: *const OdOperator,
    destination: *const c_char,
    raw_options: OdCopyOptions,
    err: *mut OdError,
) -> *mut OdCopyCursor {
    ffi_guard!(err, std::ptr::null_mut(), "od_table_copy_open", {
        if source_op.is_null() || destination_op.is_null() {
            set_error(
                err,
                OdErrorCode::InvalidInput,
                "null source or destination operator",
            );
            return std::ptr::null_mut();
        }
        let source = match cstr(source) {
            Some(source) => source,
            None => {
                set_error(
                    err,
                    OdErrorCode::InvalidInput,
                    "source is null or not UTF-8",
                );
                return std::ptr::null_mut();
            }
        };
        let destination = match cstr(destination) {
            Some(destination) => destination,
            None => {
                set_error(
                    err,
                    OdErrorCode::InvalidInput,
                    "destination is null or not UTF-8",
                );
                return std::ptr::null_mut();
            }
        };
        let mut options = match copy_options(raw_options) {
            Ok(options) => options,
            Err(message) => {
                set_error(err, OdErrorCode::InvalidInput, message);
                return std::ptr::null_mut();
            }
        };
        let source_op = &*source_op;
        let destination_op = &*destination_op;
        let source_metadata = match block_on(source_op.op.stat_options(
            source,
            StatOptions {
                version: options.source_version.clone(),
                ..Default::default()
            },
        )) {
            Ok(metadata) => metadata,
            Err(error) => {
                set_opendal_error(err, &error);
                return std::ptr::null_mut();
            }
        };
        let source_size = source_metadata.content_length();
        if let Some(hint) = options.source_content_length_hint {
            if hint != source_size {
                set_error(
                    err,
                    OdErrorCode::InvalidInput,
                    format!(
                        "source_content_length_hint={hint} does not match source size {source_size}"
                    ),
                );
                return std::ptr::null_mut();
            }
        }
        // Segmented copiers plan source ranges from this hint. Version-aware
        // stat is authoritative; validated size prevents truncated copies.
        options.source_content_length_hint = Some(source_size);
        let result: Result<(u64, Metadata), opendal::Error> =
            if std::ptr::eq(source_op, destination_op) && source_op.cap.copy {
                match block_on(
                    source_op
                        .op
                        .copier_options(source, destination, options.clone()),
                ) {
                    Ok(mut copier) => {
                        let mut copied = 0u64;
                        loop {
                            match block_on(copier.next()) {
                                Ok(Some(bytes)) => {
                                    copied = match copied.checked_add(bytes as u64) {
                                        Some(value) => value,
                                        None => {
                                            let _ = block_on(copier.abort());
                                            set_error(
                                                err,
                                                OdErrorCode::Unexpected,
                                                "copy byte count overflow",
                                            );
                                            return std::ptr::null_mut();
                                        }
                                    };
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    let _ = block_on(copier.abort());
                                    set_opendal_error(err, &error);
                                    return std::ptr::null_mut();
                                }
                            }
                        }
                        block_on(copier.close()).map(|metadata| (copied.max(source_size), metadata))
                    }
                    Err(error) => Err(error),
                }
            } else {
                if let Err((code, message)) = require(&source_op.scheme, source_op.cap.read, "read")
                {
                    set_error(err, code, message);
                    return std::ptr::null_mut();
                }
                if let Err((code, message)) =
                    require(&destination_op.scheme, destination_op.cap.write, "write")
                {
                    set_error(err, code, message);
                    return std::ptr::null_mut();
                }
                let size = source_size;
                let mut reader_options = ReaderOptions {
                    version: options.source_version.clone(),
                    content_length_hint: options.source_content_length_hint.or(Some(size)),
                    concurrent: options.concurrent,
                    chunk: options.chunk,
                    ..Default::default()
                };
                if reader_options.concurrent == 0 {
                    reader_options.concurrent = 1;
                }
                let reader = match block_on(source_op.op.reader_options(source, reader_options)) {
                    Ok(reader) => reader,
                    Err(error) => {
                        set_opendal_error(err, &error);
                        return std::ptr::null_mut();
                    }
                };
                let writer_options = WriteOptions {
                    if_not_exists: options.if_not_exists,
                    if_match: options.if_match.clone(),
                    concurrent: options.concurrent,
                    chunk: options.chunk,
                    ..Default::default()
                };
                let mut writer = match block_on(
                    destination_op
                        .op
                        .writer_options(destination, writer_options),
                ) {
                    Ok(writer) => writer,
                    Err(error) => {
                        set_opendal_error(err, &error);
                        return std::ptr::null_mut();
                    }
                };
                let chunk = options.chunk.unwrap_or(8 * 1024 * 1024) as u64;
                let mut offset = 0u64;
                while offset < size {
                    let end = size.min(offset.saturating_add(chunk));
                    let buffer = match block_on(reader.read(offset..end)) {
                        Ok(buffer) => buffer,
                        Err(error) => {
                            let error = abort_writer(&mut writer, error);
                            set_opendal_error(err, &error);
                            return std::ptr::null_mut();
                        }
                    };
                    if let Err(error) = block_on(writer.write(buffer)) {
                        let error = abort_writer(&mut writer, error);
                        set_opendal_error(err, &error);
                        return std::ptr::null_mut();
                    }
                    offset = end;
                }
                match block_on(writer.close()) {
                    Ok(metadata) => Ok((offset, metadata)),
                    Err(error) => Err(abort_writer(&mut writer, error)),
                }
            };
        let (bytes_copied, completion_metadata) = match result {
            Ok(result) => result,
            Err(error) => {
                set_opendal_error(err, &error);
                return std::ptr::null_mut();
            }
        };
        let metadata = match block_on(destination_op.op.stat(destination)) {
            Ok(metadata) => metadata,
            // Copy is committed and cannot be rolled back. Avoid reporting a
            // false operation failure when destination stat is not permitted.
            Err(_) => completion_metadata,
        };
        let row = match OwnedRow::from_parts(destination.to_owned(), metadata) {
            Ok(row) => row,
            Err(message) => {
                set_error(err, OdErrorCode::Unexpected, message);
                return std::ptr::null_mut();
            }
        };
        set_ok(err);
        Box::into_raw(Box::new(OdCopyCursor {
            row: Some((bytes_copied, row)),
            current: None,
        }))
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_copy_cursor_next(
    cursor: *mut OdCopyCursor,
    out: *mut OdCopyRow,
    err: *mut OdError,
) -> i8 {
    ffi_guard!(err, -1, "od_copy_cursor_next", {
        if cursor.is_null() || out.is_null() {
            set_error(
                err,
                OdErrorCode::InvalidInput,
                "null copy cursor or row output",
            );
            return -1;
        }
        let cursor = &mut *cursor;
        let Some(row) = cursor.row.take() else {
            set_ok(err);
            return 0;
        };
        cursor.current = Some(row);
        let (bytes_copied, row) = cursor.current.as_ref().unwrap();
        *out = OdCopyRow {
            bytes_copied: *bytes_copied,
            metadata: row.borrowed().metadata,
        };
        set_ok(err);
        1
    })
}

#[no_mangle]
pub unsafe extern "C" fn od_copy_cursor_free(cursor: *mut OdCopyCursor) {
    free_handle(cursor);
}

#[cfg(test)]
mod tests {
    use super::{copy_options, glob_matches, OdCopyOptions};

    #[test]
    fn matches_full_path_globs() {
        assert!(glob_matches(b"root/**/*.parquet", b"root/a.parquet"));
        assert!(glob_matches(
            b"root/**/*.parquet",
            b"root/part=1/data.parquet"
        ));
        assert!(glob_matches(
            b"root/part=[0-2]/*.parquet",
            b"root/part=1/a.parquet"
        ));
        assert!(!glob_matches(b"root/*.parquet", b"root/nested/a.parquet"));
        assert!(!glob_matches(
            b"root/part=[0-2]/*.parquet",
            b"root/part=9/a.parquet"
        ));
    }

    #[test]
    fn accepts_zero_content_length_hint() {
        let options = copy_options(OdCopyOptions {
            if_not_exists: 0,
            if_match: std::ptr::null(),
            source_version: std::ptr::null(),
            source_content_length_hint: 0,
            has_source_content_length_hint: 1,
            concurrent: 0,
            has_concurrent: 0,
            chunk_size: 0,
            has_chunk_size: 0,
        })
        .unwrap();
        assert_eq!(options.source_content_length_hint, Some(0));
    }
}
