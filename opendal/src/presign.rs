//! Presigned requests across the FFI boundary.

use std::ffi::{c_char, CString};
use std::time::Duration;

use opendal::options::{DeleteOptions, ReadOptions, StatOptions, WriteOptions};
use opendal::raw::PresignedRequest;

use crate::capability::require;
use crate::error::{opendal_error_code, set_error, set_ok, OdError, OdErrorCode};
use crate::ffi::{cstr, ffi_guard, free_handle};
use crate::operator::OdOperator;
use crate::runtime::block_on;

#[derive(Clone, Copy, Debug, PartialEq)]
enum PresignOperation {
    Stat,
    Read,
    Write,
    Delete,
}

impl PresignOperation {
    const ALL: [Self; 4] = [Self::Stat, Self::Read, Self::Write, Self::Delete];

    fn parse(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.name() == name)
            .ok_or_else(|| {
                let names: Vec<_> = Self::ALL.iter().map(|operation| operation.name()).collect();
                format!(
                    "operation '{name}' is not valid, use one of: {}",
                    names.join(", ")
                )
            })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Stat => "stat",
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
        }
    }

    fn capabilities(self, cap: &opendal::Capability) -> (bool, Option<bool>) {
        match self {
            Self::Stat => (cap.presign_stat, Some(cap.stat_with_version)),
            Self::Read => (cap.presign_read, Some(cap.read_with_version)),
            Self::Write => (cap.presign_write, None),
            Self::Delete => (cap.presign_delete, Some(cap.delete_with_version)),
        }
    }
}

/// A presigned request. Borrowed pointers are valid until [`od_presigned_free`].
#[repr(C)]
pub struct OdPresignedRequest {
    pub method: *const c_char,
    pub url: *const c_char,
    pub header_keys: *const *const c_char,
    pub header_values: *const *const c_char,
    pub header_len: usize,
}

pub struct OdPresigned {
    method: CString,
    url: CString,
    _header_keys: Vec<CString>,
    _header_values: Vec<CString>,
    header_key_ptrs: Vec<*const c_char>,
    header_value_ptrs: Vec<*const c_char>,
}

impl OdPresigned {
    fn new(request: &PresignedRequest) -> Result<Self, String> {
        let text = |value: &str, field: &str| {
            CString::new(value).map_err(|_| format!("presigned {field} contains a NUL byte"))
        };
        let mut header_keys = Vec::with_capacity(request.header().len());
        let mut header_values = Vec::with_capacity(request.header().len());
        for (key, value) in request.header() {
            let value = value
                .to_str()
                .map_err(|_| format!("presigned header '{key}' is not visible ASCII"))?;
            header_keys.push(text(key.as_str(), "header name")?);
            header_values.push(text(value, "header value")?);
        }
        Ok(Self {
            method: text(request.method().as_str(), "method")?,
            url: text(&request.uri().to_string(), "url")?,
            header_key_ptrs: header_keys.iter().map(|key| key.as_ptr()).collect(),
            header_value_ptrs: header_values.iter().map(|value| value.as_ptr()).collect(),
            _header_keys: header_keys,
            _header_values: header_values,
        })
    }

    fn borrowed(&self) -> OdPresignedRequest {
        OdPresignedRequest {
            method: self.method.as_ptr(),
            url: self.url.as_ptr(),
            header_keys: self.header_key_ptrs.as_ptr(),
            header_values: self.header_value_ptrs.as_ptr(),
            header_len: self.header_key_ptrs.len(),
        }
    }
}

fn presign(
    odop: &OdOperator,
    path: &str,
    operation: &str,
    expire_secs: u64,
    version: Option<String>,
) -> Result<PresignedRequest, (OdErrorCode, String)> {
    let invalid = |message: String| (OdErrorCode::InvalidInput, message);
    let operation = PresignOperation::parse(operation).map_err(invalid)?;
    if expire_secs == 0 {
        return Err(invalid("expire must be at least 1 second".to_owned()));
    }
    let expire = Duration::from_secs(expire_secs);
    let (supported, version_supported) = operation.capabilities(&odop.cap);
    require(
        &odop.scheme,
        supported,
        &format!("presign_{}", operation.name()),
    )?;
    if version.is_some() {
        let Some(version_supported) = version_supported else {
            return Err(invalid(format!(
                "version is not valid for operation '{}'",
                operation.name()
            )));
        };
        require(
            &odop.scheme,
            version_supported,
            &format!("{}_with_version", operation.name()),
        )?;
    }
    let op = &odop.op;
    let result = match operation {
        PresignOperation::Stat => block_on(op.presign_stat_options(
            path,
            expire,
            StatOptions {
                version,
                ..Default::default()
            },
        )),
        PresignOperation::Read => block_on(op.presign_read_options(
            path,
            expire,
            ReadOptions {
                version,
                ..Default::default()
            },
        )),
        PresignOperation::Write => {
            block_on(op.presign_write_options(path, expire, WriteOptions::default()))
        }
        PresignOperation::Delete => block_on(op.presign_delete_options(
            path,
            expire,
            DeleteOptions {
                version,
                ..Default::default()
            },
        )),
    };
    result.map_err(|error| (opendal_error_code(&error), error.to_string()))
}

/// Create a presigned request for `operation` on `path`.
///
/// `operation` is one of `stat`, `read`, `write` or `delete`. `version` applies
/// to `stat`, `read` and `delete`.
///
/// # Safety
///
/// - `op` must be a valid, non-null [`OdOperator`] handle.
/// - `path` and `operation` must be valid, NUL-terminated UTF-8 C strings.
/// - `version` must be null or a valid, NUL-terminated UTF-8 C string.
/// - `out` must point to a writable [`OdPresignedRequest`].
/// - `err` must point to a writable [`OdError`].
/// - The returned handle must be freed with [`od_presigned_free`].
#[no_mangle]
pub unsafe extern "C" fn od_presign(
    op: *const OdOperator,
    path: *const c_char,
    operation: *const c_char,
    expire_secs: u64,
    version: *const c_char,
    out: *mut OdPresignedRequest,
    err: *mut OdError,
) -> *mut OdPresigned {
    ffi_guard!(err, std::ptr::null_mut(), "od_presign", {
        if op.is_null() || out.is_null() {
            set_error(
                err,
                OdErrorCode::InvalidInput,
                "null operator or request output",
            );
            return std::ptr::null_mut();
        }
        let (Some(path), Some(operation)) = (cstr(path), cstr(operation)) else {
            set_error(
                err,
                OdErrorCode::InvalidInput,
                "path or operation is null or not UTF-8",
            );
            return std::ptr::null_mut();
        };
        let version = if version.is_null() {
            None
        } else {
            match cstr(version) {
                Some(version) => Some(version.to_owned()),
                None => {
                    set_error(err, OdErrorCode::InvalidInput, "version is not UTF-8");
                    return std::ptr::null_mut();
                }
            }
        };
        let request = match presign(&*op, path, operation, expire_secs, version) {
            Ok(request) => request,
            Err((code, message)) => {
                set_error(err, code, message);
                return std::ptr::null_mut();
            }
        };
        match OdPresigned::new(&request) {
            Ok(presigned) => {
                let presigned = Box::into_raw(Box::new(presigned));
                *out = (*presigned).borrowed();
                set_ok(err);
                presigned
            }
            Err(message) => {
                set_error(err, OdErrorCode::Unexpected, message);
                std::ptr::null_mut()
            }
        }
    })
}

/// # Safety
///
/// `presigned` must be null or a handle from [`od_presign`], not already freed.
#[no_mangle]
pub unsafe extern "C" fn od_presigned_free(presigned: *mut OdPresigned) {
    free_handle(presigned);
}

#[cfg(test)]
mod tests {
    use super::{presign, PresignOperation};
    use crate::error::OdErrorCode;
    use crate::operator::{od_operator_new, OdOperator};
    use std::ffi::CString;

    fn operator(uri: &str, config: &[(&str, &str)]) -> Box<OdOperator> {
        let mut err = crate::error::OdError::ok();
        assert_eq!(unsafe { crate::od_init(0, &mut err) }, 0);
        let uri = CString::new(uri).unwrap();
        let keys: Vec<_> = config
            .iter()
            .map(|(k, _)| CString::new(*k).unwrap())
            .collect();
        let values: Vec<_> = config
            .iter()
            .map(|(_, v)| CString::new(*v).unwrap())
            .collect();
        let key_ptrs: Vec<_> = keys.iter().map(|k| k.as_ptr()).collect();
        let value_ptrs: Vec<_> = values.iter().map(|v| v.as_ptr()).collect();
        let op = unsafe {
            od_operator_new(
                uri.as_ptr(),
                key_ptrs.as_ptr(),
                value_ptrs.as_ptr(),
                config.len(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut err,
            )
        };
        assert!(!op.is_null());
        unsafe { Box::from_raw(op) }
    }

    fn s3() -> Box<OdOperator> {
        operator(
            "s3://bucket",
            &[
                ("access_key_id", "key"),
                ("secret_access_key", "secret"),
                ("region", "us-east-1"),
                ("endpoint", "http://127.0.0.1:9000"),
                ("disable_config_load", "true"),
                ("disable_ec2_metadata", "true"),
            ],
        )
    }

    #[test]
    fn rejects_unknown_operation() {
        let Err(error) = PresignOperation::parse("list") else {
            panic!("presign must fail");
        };
        assert!(error.contains("stat, read, write, delete"), "{error}");
    }

    #[test]
    fn rejects_version_for_write() {
        let Err(error) = presign(&s3(), "a", "write", 60, Some("v1".into())) else {
            panic!("presign must fail");
        };
        assert_eq!(error.0 as i32, OdErrorCode::InvalidInput as i32);
    }

    #[test]
    fn rejects_service_without_capability() {
        let Err(error) = presign(&operator("memory://", &[]), "a", "read", 60, None) else {
            panic!("presign must fail");
        };
        assert_eq!(error.0 as i32, OdErrorCode::Unsupported as i32);
        assert!(
            error.1.contains("does not support presign_read"),
            "{}",
            error.1
        );
    }

    #[test]
    fn signs_each_operation_with_version() {
        let op = s3();
        for (operation, method) in [("stat", "HEAD"), ("read", "GET"), ("delete", "DELETE")] {
            let request = presign(&op, "a", operation, 900, Some("v1".into()))
                .unwrap_or_else(|(_, message)| panic!("{message}"));
            let url = request.uri().to_string();
            assert_eq!(request.method().as_str(), method);
            assert!(url.contains("versionId=v1"), "{url}");
            assert!(url.contains("X-Amz-Expires=900"), "{url}");
        }
        let request = presign(&op, "a", "write", 900, None)
            .unwrap_or_else(|(_, message)| panic!("{message}"));
        assert_eq!(request.method().as_str(), "PUT");
    }
}
