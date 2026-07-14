//! Rust-owned table scans for OpenDAL entries and metadata.

use std::collections::HashMap;
use std::ffi::{c_char, CString};

use futures::TryStreamExt;
use opendal::options::{ListOptions, StatOptions};
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

struct OwnedDuRow {
    directory: CString,
    file_count: u64,
    total_size: u64,
}

pub struct OdDuCursor {
    rows: std::vec::IntoIter<OwnedDuRow>,
    current: Option<OwnedDuRow>,
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

#[cfg(test)]
mod tests {
    use super::glob_matches;

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
}
